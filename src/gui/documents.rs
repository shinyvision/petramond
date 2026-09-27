use super::GuiKind;
use crate::menu::slots::{doc_container_specs, ItemTags, DOCUMENTS_DIR};
use petramond_ui::contract::{self, image_refs, validate_for_engine, EngineCheck};
use petramond_ui::{DocClass, Document, Node, NodeKind, SlotContract};
use petramond_world::container::{MAX_CONTAINER_SLOTS, MAX_SLOT_FILTERS};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub struct DocEntry {
    pub kind: GuiKind,
    pub doc: Arc<Document>,
    pub images: Arc<Vec<DocImageRef>>,
}

#[derive(Clone, Debug)]
pub struct DocImageRef {
    pub name: String,
    pub path: PathBuf,
    pub size: (u32, u32),
}

#[derive(Clone)]
pub struct DocRef {
    pub doc: Arc<Document>,
    pub images: Arc<Vec<DocImageRef>>,
}

struct Registry {
    entries: Vec<DocEntry>,
    sources: Vec<(PathBuf, Option<SystemTime>)>,
    last_check: Instant,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

pub fn reload() {
    *REGISTRY.lock().unwrap() = None;
}

pub fn doc_for(kind: GuiKind) -> Option<DocRef> {
    doc_entry_for(kind).or_else(|| {
        use petramond_world::crafting::CraftingStation;
        CraftingStation::of_kind(kind)
            .is_some_and(|s| s != CraftingStation::Inventory && s != CraftingStation::CraftingTable)
            .then(|| doc_entry_for(GuiKind::CraftingTable))
            .flatten()
    })
}

fn doc_entry_for(kind: GuiKind) -> Option<DocRef> {
    let mut guard = REGISTRY.lock().unwrap();
    let registry = guard.get_or_insert_with(load);
    if cfg!(debug_assertions) && registry.last_check.elapsed() > Duration::from_secs(1) {
        let changed = registry
            .sources
            .iter()
            .any(|(path, mtime)| file_mtime(path) != *mtime);
        if changed {
            *registry = load();
        } else {
            registry.last_check = Instant::now();
        }
    }
    registry
        .entries
        .iter()
        .find(|e| e.kind == kind)
        .map(|e| DocRef {
            doc: e.doc.clone(),
            images: e.images.clone(),
        })
}

pub fn loaded_documents() -> Vec<(&'static str, usize)> {
    let mut guard = REGISTRY.lock().expect("gui document registry");
    let registry = guard.get_or_insert_with(load);
    let mut out: Vec<(&'static str, usize)> = registry
        .entries
        .iter()
        .filter_map(|e| {
            Some((
                petramond_world::gui_state::kind_key(e.kind)?,
                crate::menu::slot_specs_for_kind(e.kind).len(),
            ))
        })
        .collect();
    out.sort_unstable();
    out
}

const _: () = {
    use petramond_ui::contract as ui;
    use petramond_world::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
    assert!(ui::CHEST_SLOTS == crate::world::chest::CHEST_SLOTS);
    assert!(ui::FURNACE_SLOTS == petramond_world::furnace::FURNACE_SLOTS);
    assert!(ui::HOTBAR_SLOTS == HOTBAR_LEN);
    assert!(ui::MAIN_GRID_SLOTS == TOTAL_SLOTS - HOTBAR_LEN);
    assert!(ui::MAX_CONTAINER_SLOTS == MAX_CONTAINER_SLOTS);
    assert!(ui::MAX_SLOT_FILTERS == MAX_SLOT_FILTERS);
    assert!(ui::IMAGE_MAX_SIDE == mod_api::GUI_IMAGE_MAX_SIDE);
    assert!(ui::IMAGE_MAX_FRAMES == mod_api::GUI_IMAGE_MAX_FRAMES);
};

pub fn contract_for(kind: GuiKind) -> SlotContract {
    super::kind_key(kind)
        .and_then(contract::engine_kind)
        .map(|k| k.contract())
        .unwrap_or_default()
}

fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn image_size_beside(dir: &std::path::Path, name: &str) -> Option<(u32, u32)> {
    image::image_dimensions(dir.join(name)).ok()
}

fn collect_doc_images(doc: &Document, dir: &std::path::Path) -> Result<Vec<DocImageRef>, String> {
    let refs = image_refs(doc);
    let mut images = Vec::with_capacity(refs.len());
    for r in refs {
        let size = image_size_beside(dir, &r.name)
            .ok_or_else(|| format!("names missing art {}", r.name))?;
        if let Some(frames) = r.frames {
            contract::check_frame_sheet(&r.name, size, frames)?;
        }
        images.push(DocImageRef {
            path: dir.join(&r.name),
            name: r.name,
            size,
        });
    }
    Ok(images)
}

fn inject_item_tooltip(doc: &mut Document) {
    const STANDARD: &str = r#"{
        "type": "tooltip",
        "style": "panel.inset",
        "layout": { "abs": { "x": 4, "y": 4 }, "pad": [3, 2, 3, 3], "gap": 2, "align": "stretch" },
        "bind": { "visible": "show_item_tip" },
        "children": [
            { "type": "label", "small": true, "wrap": true, "bind": { "text": "item_tip_name" } },
            { "type": "label", "small": true, "style": "label.muted", "wrap": true,
              "bind": { "text": "item_tip_info", "visible": "item_tip_has_info" } },
            { "type": "label", "small": true, "wrap": true,
              "bind": { "text": "item_tip_instance", "visible": "item_tip_instance" } },
            { "type": "row", "layout": { "gap": 4 }, "children": [
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon0", "visible": "item_tip_icon0" } },
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon1", "visible": "item_tip_icon1" } },
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon2", "visible": "item_tip_icon2" } },
                { "type": "image", "image": "", "layout": { "w": 16, "h": 16 },
                  "bind": { "image": "item_tip_icon3", "visible": "item_tip_icon3" } }
            ] }
        ]
    }"#;
    fn binds_item_tip(node: &Node) -> bool {
        node.bind.visible.as_deref() == Some("show_item_tip")
            || node.children.iter().any(binds_item_tip)
    }
    if doc.class != DocClass::Container || binds_item_tip(&doc.root) {
        return;
    }
    let mut node: Node = serde_json::from_str(STANDARD).expect("standard item tooltip node parses");
    node.layout.max_w = Some(ITEM_TIP_MAX_W);
    doc.root.children.push(node);
}

pub const ITEM_TIP_MAX_W: i32 = 200;

pub const SLOT_TIP_MAX_W: i32 = 240;

pub const SLOT_TIP_LINES: usize = 3;
pub const SLOT_TIP_SPANS: usize = 2;

pub fn slot_tip_keys(line: usize, span: usize) -> (String, String) {
    (
        format!("slot_tip_l{line}s{span}"),
        format!("slot_tip_c{line}s{span}"),
    )
}

pub const SHOW_SLOT_TIP: &str = "show_slot_tip";

fn inject_slot_tooltip(doc: &mut Document) {
    const CHROME: &str = r#"{
        "type": "tooltip",
        "style": "panel.inset",
        "layout": { "abs": { "x": 4, "y": 4 }, "pad": [3, 2, 3, 3], "gap": 2, "align": "stretch" },
        "bind": { }
    }"#;
    fn binds_slot_tip(node: &Node) -> bool {
        node.bind.visible.as_deref() == Some(SHOW_SLOT_TIP)
            || node.children.iter().any(binds_slot_tip)
    }
    if doc.class != DocClass::Container || binds_slot_tip(&doc.root) {
        return;
    }
    let mut tip: Node = serde_json::from_str(CHROME).expect("standard slot tooltip node parses");
    tip.layout.max_w = Some(SLOT_TIP_MAX_W);
    tip.bind.visible = Some(SHOW_SLOT_TIP.to_owned());
    tip.children = (0..SLOT_TIP_LINES)
        .map(|line| {
            let mut row = Node::leaf(NodeKind::Row);
            row.bind.visible = Some(slot_tip_keys(line, 0).0);
            row.children = (0..SLOT_TIP_SPANS)
                .map(|span| slot_tip_span(line, span))
                .collect();
            row
        })
        .collect();
    doc.root.children.push(tip);
}

fn slot_tip_span(line: usize, span: usize) -> Node {
    let (text, palette) = slot_tip_keys(line, span);
    let mut label = Node::leaf(NodeKind::Label {
        text: None,
        wrap: false,
        scale: 1,
        small: true,
        max_lines: None,
    });
    label.bind.text = Some(text);
    label.bind.palette = Some(palette);
    label
}

fn load() -> Registry {
    struct Found {
        json: PathBuf,
        dir: PathBuf,
        pack_id: Option<String>,
    }
    let mut manifests: Vec<(String, Found)> = Vec::new();
    let mut sources: Vec<(PathBuf, Option<SystemTime>)> = Vec::new();
    for (dir, pack_id) in petramond_world::assets::layer_dirs_with_ids(DOCUMENTS_DIR) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".gui.json") {
                continue;
            }
            let found = Found {
                json: path,
                dir: dir.clone(),
                pack_id: pack_id.clone(),
            };
            match manifests.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = found,
                None => manifests.push((name, found)),
            }
        }
    }
    manifests.sort_by(|a, b| a.0.cmp(&b.0));

    let theme = super::doc_theme::theme();
    let mut entries = Vec::new();
    for (_, found) in manifests {
        sources.push((found.json.clone(), file_mtime(&found.json)));
        match load_entry(&found.json, &found.dir, found.pack_id.as_deref(), &theme) {
            Ok(entry) => entries.push(entry),
            Err(e) => log::warn!("gui: ignoring {} — {e}", found.json.display()),
        }
    }
    Registry {
        entries,
        sources,
        last_check: Instant::now(),
    }
}

fn load_entry(
    json: &std::path::Path,
    dir: &std::path::Path,
    pack_id: Option<&str>,
    theme: &petramond_ui::Theme,
) -> Result<DocEntry, String> {
    let (kind, doc, images) = read_document(json, dir, pack_id, theme)?;
    let declared = doc_container_specs(&doc)?.len();
    let contracted = crate::menu::slot_specs_for_kind(kind).len();
    if declared != 0 && declared != contracted {
        return Err(format!(
            "{declared} container slots, but the kind's contract has {contracted}"
        ));
    }
    Ok(DocEntry {
        kind,
        doc: Arc::new(doc),
        images: Arc::new(images),
    })
}

fn read_document(
    json: &std::path::Path,
    dir: &std::path::Path,
    pack_id: Option<&str>,
    theme: &petramond_ui::Theme,
) -> Result<(GuiKind, Document, Vec<DocImageRef>), String> {
    let text = std::fs::read_to_string(json).map_err(|e| e.to_string())?;
    let mut doc = Document::from_json(&text).map_err(|e| e.to_string())?;
    inject_item_tooltip(&mut doc);
    inject_slot_tooltip(&mut doc);
    let kind =
        super::intern_kind(&doc.kind).ok_or_else(|| format!("unknown kind '{}'", doc.kind))?;
    let issues = validate_for_engine(
        &doc,
        &EngineCheck {
            styles: Some(theme),
            pack_id,
            catalog: &ItemTags,
            image_size: &|name| image_size_beside(dir, name),
        },
    );
    if !issues.is_empty() {
        let issues: Vec<String> = issues.iter().map(ToString::to_string).collect();
        return Err(issues.join("; "));
    }
    let images = collect_doc_images(&doc, dir)?;
    Ok((kind, doc, images))
}

#[cfg(test)]
mod tests;
