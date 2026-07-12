//! Best-effort importer for legacy (layer-compositor) `.llgui` files — the
//! pre-document format the old builder baked to PNGs. It produces a *starting
//! point* document, not whole-screen fidelity: slot grids become
//! `slot`/`slot_grid` nodes with roles, shell buttons/inputs become widgets,
//! file-sourced background layers become abs-positioned `image` nodes, and
//! anything untranslatable becomes a `TODO:` label the author replaces.

use crate::doc_edit;
use petramond_ui::{
    AbsPos, Anchor, AnchorEdge, DocClass, Document, LayoutProps, Node, NodeKind, Size,
    FORMAT_VERSION,
};
use serde::Deserialize;

// ---- minimal legacy model (deserialize only) ---------------------------------

#[derive(Deserialize)]
struct LegacyProject {
    gui_type: String,
    canvas: LegacyCanvas,
    #[serde(default)]
    nodes: Vec<LegacyNode>,
    #[serde(default)]
    slots: Vec<LegacySlot>,
}

#[derive(Deserialize)]
struct LegacyCanvas {
    w: u32,
    h: u32,
}

#[derive(Deserialize)]
#[serde(tag = "node", rename_all = "snake_case")]
enum LegacyNode {
    Layer(LegacyLayer),
    Group(LegacyGroup),
}

#[derive(Deserialize)]
struct LegacyLayer {
    name: String,
    asset: LegacyAsset,
    rect: LegacyRect,
    #[serde(default = "yes")]
    visible: bool,
}

#[derive(Deserialize)]
struct LegacyGroup {
    #[serde(default = "yes")]
    visible: bool,
    #[serde(default)]
    children: Vec<LegacyNode>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
enum LegacyAsset {
    Builtin { key: String },
    File { path: std::path::PathBuf },
}

#[derive(Clone, Copy, Deserialize)]
struct LegacyRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Deserialize)]
struct LegacySlot {
    role: String,
    rect: LegacyRect,
    #[serde(default)]
    grid: Option<LegacyGrid>,
    #[serde(default = "yes")]
    visible: bool,
}

#[derive(Deserialize)]
struct LegacyGrid {
    cols: u32,
    rows: u32,
}

// ---- conversion ---------------------------------------------------------------

pub struct Imported {
    pub document: Document,
    pub warnings: Vec<String>,
}

pub fn import(json: &str) -> Result<Imported, String> {
    let legacy: LegacyProject =
        serde_json::from_str(json).map_err(|e| format!("legacy .llgui: {e}"))?;
    Ok(convert(&legacy))
}

fn convert(legacy: &LegacyProject) -> Imported {
    let mut warnings = Vec::new();
    let (kind, class) = kind_of(&legacy.gui_type);
    let mut root = Node::leaf(NodeKind::Frame);
    root.style = Some("panel.large".into());
    root.layout = LayoutProps {
        w: Size::Px(legacy.canvas.w as i32),
        h: Size::Px(legacy.canvas.h as i32),
        anchor: Some(Anchor {
            h: AnchorEdge::Center,
            v: if class == DocClass::Hud { AnchorEdge::End } else { AnchorEdge::Center },
        }),
        ..LayoutProps::default()
    };

    // Layers, flattened back-to-front. The first full-canvas builtin layer is
    // the old baked background — the root panel replaces it.
    let mut background_skipped = false;
    let mut layers = Vec::new();
    flatten(&legacy.nodes, true, &mut layers);
    for (layer, visible) in layers {
        if !visible {
            continue;
        }
        match &layer.asset {
            LegacyAsset::Builtin { key } => {
                let full_canvas = layer.rect.x == 0
                    && layer.rect.y == 0
                    && layer.rect.w == legacy.canvas.w as i32
                    && layer.rect.h == legacy.canvas.h as i32;
                if full_canvas && !background_skipped {
                    background_skipped = true;
                    continue; // the panel.large root frame stands in for it
                }
                warnings.push(format!(
                    "layer '{}' uses builtin asset '{key}' — left as a TODO label",
                    layer.name
                ));
                root.children.push(todo_label(
                    format!("TODO: legacy layer '{}' ({key})", layer.name),
                    layer.rect,
                ));
            }
            LegacyAsset::File { path } => {
                let name = path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                let mut node = Node::leaf(NodeKind::Image { image: name, fit: Default::default(), interactive: false });
                node.layout = abs_layout(layer.rect, true);
                root.children.push(node);
            }
        }
    }

    // Slots. Old shell projects modeled buttons/inputs/rows as typed "slots";
    // real item-slot roles become slot/slot_grid nodes.
    let doc_stub = Document {
        format: FORMAT_VERSION,
        kind: kind.clone(),
        class,
        compact_below_w: None,
        root: Node::leaf(NodeKind::Frame),
    };
    let mut ids_doc = doc_stub; // grows ids as we mint them
    for slot in &legacy.slots {
        if !slot.visible {
            continue;
        }
        let (cols, rows) = slot.grid.as_ref().map(|g| (g.cols, g.rows)).unwrap_or((1, 1));
        let node = match slot.role.as_str() {
            // Item-slot roles pass straight through.
            "generic" | "storage" | "player_inv" | "hotbar" | "craft_result"
            | "furnace_input" | "furnace_fuel" | "furnace_output" | "workbench_input"
            | "workbench_result" => {
                let kind = if cols * rows <= 1 {
                    NodeKind::Slot { role: slot.role.clone(), accepts: Vec::new(), take_only: false }
                } else {
                    NodeKind::SlotGrid { role: slot.role.clone(), cols, rows, accepts: Vec::new(), take_only: false }
                };
                let mut n = Node::leaf(kind);
                n.layout = abs_layout(slot.rect, false);
                n
            }
            // Shell buttons.
            role if button_label(role).is_some() => {
                let (id, text) = button_label(role).unwrap();
                let mut n = Node::leaf(NodeKind::Button { text: Some(text.to_owned()), icon: None });
                n.id = Some(doc_edit::unique_id(&ids_doc, id));
                n.layout = abs_layout(slot.rect, true);
                n
            }
            // Shell text inputs.
            "create_name_input" | "create_seed_input" => {
                let id = slot.role.trim_end_matches("_input");
                let mut n = Node::leaf(NodeKind::TextInput { placeholder: None, max_chars: 64 });
                n.id = Some(doc_edit::unique_id(&ids_doc, id));
                n.layout = abs_layout(slot.rect, true);
                n
            }
            other => {
                warnings.push(format!(
                    "slot role '{other}' has no document equivalent — left as a TODO label"
                ));
                todo_label(format!("TODO: legacy slot '{other}'"), slot.rect)
            }
        };
        if let Some(id) = &node.id {
            ids_doc.root.children.push({
                let mut marker = Node::leaf(NodeKind::Spacer);
                marker.id = Some(id.clone());
                marker
            });
        }
        root.children.push(node);
    }

    Imported {
        document: Document { format: FORMAT_VERSION, kind, class, compact_below_w: None, root },
        warnings,
    }
}

fn flatten<'a>(nodes: &'a [LegacyNode], visible: bool, out: &mut Vec<(&'a LegacyLayer, bool)>) {
    for n in nodes {
        match n {
            LegacyNode::Layer(l) => out.push((l, visible && l.visible)),
            LegacyNode::Group(g) => flatten(&g.children, visible && g.visible, out),
        }
    }
}

fn abs_layout(rect: LegacyRect, sized: bool) -> LayoutProps {
    LayoutProps {
        w: if sized { Size::Px(rect.w) } else { Size::Auto },
        h: if sized { Size::Px(rect.h) } else { Size::Auto },
        abs: Some(AbsPos { x: rect.x, y: rect.y }),
        ..LayoutProps::default()
    }
}

fn todo_label(text: String, rect: LegacyRect) -> Node {
    let mut n = Node::leaf(NodeKind::Label { text: Some(text), wrap: false, scale: 1 });
    n.layout = abs_layout(rect, false);
    n
}

fn kind_of(gui_type: &str) -> (String, DocClass) {
    let kind = match gui_type {
        "chest" | "inventory" | "crafting_table" | "furnace" | "hotbar"
        | "furniture_workbench" | "title" | "world_select" | "world_settings" | "create_world"
        | "delete_world" | "pause" => format!("petramond:{gui_type}"),
        _ => "custom:imported".to_owned(),
    };
    let class = crate::contracts::class_for(&kind);
    (kind, class)
}

fn button_label(role: &str) -> Option<(&'static str, &'static str)> {
    Some(match role {
        "title_start" => ("start", "START"),
        "title_quit" => ("quit", "QUIT"),
        "world_play" => ("play", "PLAY"),
        "world_create" => ("create", "CREATE NEW WORLD"),
        "world_delete" => ("delete", "DELETE WORLD"),
        "world_back" => ("back", "BACK"),
        "settings_manage_mods" => ("manage_mods", "MANAGE MODS"),
        "settings_back" => ("back", "BACK"),
        "settings_delete_world" => ("delete_world", "DELETE WORLD"),
        "create_create" => ("create", "CREATE"),
        "create_cancel" => ("cancel", "CANCEL"),
        "delete_world_confirm" => ("confirm", "DELETE"),
        "delete_world_cancel" => ("cancel", "CANCEL"),
        "pause_resume" => ("resume", "RESUME"),
        "pause_save_quit" => ("save_quit", "SAVE & QUIT"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_PAUSE: &str = r#"{
        "version": 2, "gui_type": "pause", "scale": 1,
        "canvas": { "w": 218, "h": 116 },
        "nodes": [
            { "node": "layer", "id": 1, "name": "Background",
              "asset": { "source": "builtin", "key": "shell_panel" },
              "rect": { "x": 0, "y": 0, "w": 218, "h": 116 },
              "fit": { "mode": "nine_slice", "l": 8, "r": 8, "t": 8, "b": 8 },
              "opacity": 1.0, "visible": true },
            { "node": "group", "id": 9, "name": "Deco", "visible": true, "children": [
                { "node": "layer", "id": 10, "name": "Logo",
                  "asset": { "source": "file", "path": "/art/logo.png" },
                  "rect": { "x": 80, "y": 8, "w": 58, "h": 20 },
                  "fit": { "mode": "stretch" }, "opacity": 1.0, "visible": true }
            ] }
        ],
        "slots": [
            { "id": 3, "role": "pause_resume", "rect": { "x": 21, "y": 57, "w": 176, "h": 20 },
              "grid": { "cols": 1, "rows": 1, "pitch_x": 18, "pitch_y": 18 },
              "color": [0,0,0,96], "paint_frame": false, "visible": true },
            { "id": 5, "role": "pause_save_quit", "rect": { "x": 21, "y": 84, "w": 176, "h": 20 },
              "grid": { "cols": 1, "rows": 1, "pitch_x": 18, "pitch_y": 18 },
              "color": [0,0,0,96], "paint_frame": false, "visible": true }
        ]
    }"#;

    const LEGACY_CHEST: &str = r#"{
        "version": 2, "gui_type": "chest", "scale": 1,
        "canvas": { "w": 176, "h": 166 },
        "nodes": [],
        "slots": [
            { "id": 1, "role": "storage", "rect": { "x": 7, "y": 17, "w": 18, "h": 18 },
              "grid": { "cols": 9, "rows": 3, "pitch_x": 18, "pitch_y": 18 },
              "color": [0,0,0,96], "paint_frame": false, "visible": true },
            { "id": 2, "role": "player_inv", "rect": { "x": 7, "y": 83, "w": 18, "h": 18 },
              "grid": { "cols": 9, "rows": 3, "pitch_x": 18, "pitch_y": 18 },
              "color": [0,0,0,96], "paint_frame": false, "visible": true },
            { "id": 3, "role": "hotbar", "rect": { "x": 7, "y": 141, "w": 18, "h": 18 },
              "grid": { "cols": 9, "rows": 1, "pitch_x": 18, "pitch_y": 18 },
              "color": [0,0,0,96], "paint_frame": false, "visible": true }
        ]
    }"#;

    #[test]
    fn pause_import_produces_a_valid_screen_document() {
        let imp = import(LEGACY_PAUSE).unwrap();
        let d = &imp.document;
        assert_eq!(d.kind, "petramond:pause");
        assert_eq!(d.class, DocClass::Screen);
        // Buttons carry ids + text; the file layer became an image node; the
        // builtin background was absorbed by the root panel.
        let types: Vec<_> = d.root.children.iter().map(|n| n.kind.type_name()).collect();
        assert_eq!(types, vec!["image", "button", "button"]);
        assert_eq!(d.root.children[1].id.as_deref(), Some("resume"));
        // Round-trips through the runtime parser and validates structurally.
        let json = d.to_json_pretty();
        let parsed = Document::from_json(&json).unwrap();
        assert_eq!(parsed.validate(None, None), vec![]);
    }

    #[test]
    fn chest_import_satisfies_the_engine_contract() {
        let imp = import(LEGACY_CHEST).unwrap();
        let contract = crate::contracts::contract_for("petramond:chest");
        let issues = imp.document.validate(None, Some(&contract));
        assert!(issues.is_empty(), "{issues:?}");
        assert!(imp.warnings.is_empty(), "{:?}", imp.warnings);
    }

    #[test]
    fn untranslatable_roles_become_todo_labels_with_warnings() {
        let json = r#"{
            "version": 2, "gui_type": "world_select", "scale": 1,
            "canvas": { "w": 276, "h": 188 }, "nodes": [],
            "slots": [
                { "id": 1, "role": "world_row", "rect": { "x": 18, "y": 37, "w": 222, "h": 16 },
                  "grid": { "cols": 1, "rows": 1, "pitch_x": 18, "pitch_y": 18 },
                  "color": [0,0,0,96], "paint_frame": false, "visible": true }
            ]
        }"#;
        let imp = import(json).unwrap();
        assert_eq!(imp.warnings.len(), 1);
        let label = &imp.document.root.children[0];
        assert_eq!(label.kind.type_name(), "label");
        match &label.kind {
            NodeKind::Label { text, .. } => {
                assert!(text.as_deref().unwrap().starts_with("TODO:"))
            }
            _ => unreachable!(),
        }
    }
}
