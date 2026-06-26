//! Data-driven GUI layouts baked by the `gui-builder` tool: a PNG panel + a JSON
//! manifest of typed slots (+ an optional hover-highlight PNG). This is the
//! forward path — every screen will be data-driven soon. The hand-coded
//! [`super::ui`] screens are the legacy system, kept as the fallback for any
//! screen kind without a (valid) manifest, and removed kind-by-kind as each is
//! baked.
//!
//! Baked GUIs are loaded from a directory at RUNTIME (not embedded) so re-baking
//! from the gui-builder + restarting the game picks them up with no recompile,
//! and so an optional hover PNG can be present-or-not without a build error. The
//! dir is resolved at compile time relative to the crate root, so it works
//! regardless of the working directory.
//!
//! The model is generic over [`GuiKind`] and slot [`Role`]: a [`GuiDef`] holds
//! the logical panel size + per-role slot rects, and both the renderer
//! ([`super::ui::build_ui`]) and the App's hit-tests read the SAME def so render
//! and click never diverge.

use super::ui::{gui_scale, SlotRect};
use crate::inventory::{HOTBAR_LEN, TOTAL_SLOTS};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Where baked GUIs live. Absolute (baked at compile from the crate root) so the
/// game finds them no matter the CWD; the gui-builder bakes into this folder.
const BAKED_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/textures/gui/baked");

static REGISTRY: OnceLock<Vec<Loaded>> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(true);

/// One baked GUI: its parsed def plus the resolved on-disk paths the renderer
/// loads its panel (and optional hover) textures from.
struct Loaded {
    def: GuiDef,
    panel_path: PathBuf,
    hover_path: Option<PathBuf>,
}

fn load_baked() -> Vec<Loaded> {
    let dir = Path::new(BAKED_DIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out; // no baked dir => no data-driven GUIs => all legacy.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Some(def) = GuiDef::from_manifest(&text) else { continue };
        let panel_path = dir.join(&def.image);
        if !panel_path.exists() {
            continue; // a manifest with no panel art is unusable.
        }
        let hover_path = def.hover.as_ref().map(|h| dir.join(&h.image)).filter(|p| p.exists());
        out.push(Loaded { def, panel_path, hover_path });
    }
    out
}

fn registry() -> &'static [Loaded] {
    REGISTRY.get_or_init(load_baked)
}

/// The def for `kind`, IF data-driven GUI is enabled AND its manifest loaded.
/// `None` => that screen falls back to the legacy hand-coded layout.
pub(crate) fn def(kind: GuiKind) -> Option<&'static GuiDef> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    registry().iter().find(|l| l.def.kind == kind).map(|l| &l.def)
}

/// (kind, panel PNG path) for every baked GUI — the renderer uploads each into a
/// texture + bind group keyed by kind.
pub(crate) fn baked_panels() -> Vec<(GuiKind, PathBuf)> {
    registry().iter().map(|l| (l.def.kind, l.panel_path.clone())).collect()
}

/// (kind, hover PNG path) for every baked GUI that has a hover highlight.
pub(crate) fn baked_hovers() -> Vec<(GuiKind, PathBuf)> {
    registry().iter().filter_map(|l| l.hover_path.clone().map(|p| (l.def.kind, p))).collect()
}

pub fn data_driven_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Toggle the data-driven GUI at runtime; legacy is used when off.
pub fn set_data_driven_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

// --- Per-kind App-facing hit-tests (one pair per container until legacy is
// removed; each is a thin wrapper over the generic role/inventory hit-tests). ---

pub fn chest_active() -> bool {
    def(GuiKind::Chest).is_some()
}

/// Chest storage slot (`0..27`) under the cursor in the data-driven layout.
pub fn chest_storage_at_cursor(screen: (u32, u32), cursor: (f32, f32)) -> Option<usize> {
    def(GuiKind::Chest)?.role_at(Role::Storage, screen, cursor)
}

/// Inventory slot (`0..36`) under the cursor in the data-driven chest layout.
pub fn chest_inventory_at_cursor(screen: (u32, u32), cursor: (f32, f32)) -> Option<usize> {
    def(GuiKind::Chest)?.inventory_at(screen, cursor)
}

// ---- manifest JSON (the gui-builder bake format) --------------------------

/// Which container a baked GUI is for. Matches the gui-builder's `type` field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GuiKind {
    Chest,
    Inventory,
    CraftingTable,
    Furnace,
    Hotbar,
    #[serde(other)]
    Other,
}

/// A slot's purpose. Matches the gui-builder's slot `role` field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Role {
    Generic,
    Storage,
    PlayerInv,
    Hotbar,
    CraftInput,
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(rename = "type")]
    kind: GuiKind,
    canvas: CanvasJson,
    scale: u32,
    image: String,
    slots: Vec<SlotJson>,
    #[serde(default)]
    hover: Option<HoverJson>,
}

#[derive(Deserialize)]
struct CanvasJson {
    w: u32,
    h: u32,
}

#[derive(Deserialize)]
struct SlotJson {
    role: Role,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Deserialize)]
struct HoverJson {
    image: String,
    margin: i32,
    #[serde(default = "default_opacity")]
    opacity: f32,
    // `fit` is present in the manifest but the game draws the highlight stretched
    // for now; serde ignores the unlisted key.
}

fn default_opacity() -> f32 {
    1.0
}

/// The hover highlight for a GUI: the graphic + how far it extends beyond a slot.
pub(crate) struct HoverDef {
    /// Base px the highlight extends beyond the slot on every side.
    pub margin: i32,
    pub opacity: f32,
    /// Panel-relative PNG filename (resolved to a path in [`load_baked`]).
    image: String,
}

/// A parsed data-driven GUI: logical panel size (canvas ÷ authoring scale) plus
/// per-role slot rects in *logical* (base) pixels — ready to centre + scale by
/// the game's integer `gui_scale`, exactly like the legacy panels.
pub(crate) struct GuiDef {
    kind: GuiKind,
    logical_w: f32,
    logical_h: f32,
    roles: HashMap<Role, Vec<[f32; 4]>>,
    /// Panel PNG filename (resolved to a path in [`load_baked`]).
    image: String,
    hover: Option<HoverDef>,
}

impl GuiDef {
    fn from_manifest(s: &str) -> Option<GuiDef> {
        let m: Manifest = serde_json::from_str(s).ok()?;
        let scale = m.scale.max(1) as f32;
        // Manifest coords are baked-canvas pixels (authoring scale); convert to
        // logical/base pixels so the game applies its own gui_scale on top.
        let mut roles: HashMap<Role, Vec<[f32; 4]>> = HashMap::new();
        for s in &m.slots {
            roles
                .entry(s.role)
                .or_default()
                .push([s.x as f32 / scale, s.y as f32 / scale, s.w as f32 / scale, s.h as f32 / scale]);
        }
        let hover = m.hover.map(|h| HoverDef {
            margin: (h.margin as f32 / scale).round() as i32,
            opacity: h.opacity,
            image: h.image,
        });
        Some(GuiDef {
            kind: m.kind,
            logical_w: m.canvas.w as f32 / scale,
            logical_h: m.canvas.h as f32 / scale,
            roles,
            image: m.image,
            hover,
        })
    }

    fn placement(&self, screen: (u32, u32)) -> (f32, f32, f32) {
        let s = gui_scale(screen);
        let (w, h) = (screen.0 as f32, screen.1 as f32);
        ((w - self.logical_w * s) * 0.5, (h - self.logical_h * s) * 0.5, s)
    }

    fn rect(&self, base: [f32; 4], screen: (u32, u32)) -> SlotRect {
        let (ox, oy, s) = self.placement(screen);
        SlotRect { x: ox + base[0] * s, y: oy + base[1] * s, w: base[2] * s, h: base[3] * s }
    }

    fn role_slots(&self, role: Role) -> &[[f32; 4]] {
        self.roles.get(&role).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub(crate) fn panel_rect(&self, screen: (u32, u32)) -> SlotRect {
        self.rect([0.0, 0.0, self.logical_w, self.logical_h], screen)
    }

    pub(crate) fn hover(&self) -> Option<&HoverDef> {
        self.hover.as_ref()
    }

    pub(crate) fn storage_rect(&self, i: usize, screen: (u32, u32)) -> Option<SlotRect> {
        self.role_slots(Role::Storage).get(i).map(|b| self.rect(*b, screen))
    }

    /// Inventory slot `i` (`0..36`): the hotbar row (`i < 9`) then the main 3×9
    /// grid, matching the game's inventory index convention.
    pub(crate) fn inventory_rect(&self, i: usize, screen: (u32, u32)) -> Option<SlotRect> {
        let base = if i < HOTBAR_LEN {
            self.role_slots(Role::Hotbar).get(i)
        } else {
            self.role_slots(Role::PlayerInv).get(i - HOTBAR_LEN)
        };
        base.map(|b| self.rect(*b, screen))
    }

    /// The screen rect of the slot under the cursor (storage or inventory) — used
    /// to place the hover highlight. The highlight inflates this by its margin.
    pub(crate) fn hovered_slot_rect(&self, screen: (u32, u32), cursor: (f32, f32)) -> Option<SlotRect> {
        let (px, py) = cursor;
        for b in self.role_slots(Role::Storage) {
            let r = self.rect(*b, screen);
            if r.contains(px, py) {
                return Some(r);
            }
        }
        (0..TOTAL_SLOTS).find_map(|i| self.inventory_rect(i, screen).filter(|r| r.contains(px, py)))
    }

    fn role_at(&self, role: Role, screen: (u32, u32), cursor: (f32, f32)) -> Option<usize> {
        let s = self.role_slots(role);
        (0..s.len()).find(|&i| self.rect(s[i], screen).contains(cursor.0, cursor.1))
    }

    fn inventory_at(&self, screen: (u32, u32), cursor: (f32, f32)) -> Option<usize> {
        (0..TOTAL_SLOTS)
            .find(|&i| self.inventory_rect(i, screen).is_some_and(|r| r.contains(cursor.0, cursor.1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Inline sample so tests don't pin the user's freely-re-baked chest.json.
    const SAMPLE: &str = r#"{
        "type": "chest",
        "canvas": { "w": 352, "h": 332 },
        "scale": 2,
        "image": "chest.png",
        "slots": [
            { "role": "storage", "x": 16, "y": 26, "w": 32, "h": 32 },
            { "role": "storage", "x": 52, "y": 26, "w": 32, "h": 32 },
            { "role": "hotbar", "x": 16, "y": 300, "w": 32, "h": 32 },
            { "role": "player_inv", "x": 16, "y": 200, "w": 32, "h": 32 }
        ],
        "hover": { "image": "chest_hover.png", "margin": 8, "fit": { "mode": "stretch" }, "opacity": 0.5 }
    }"#;

    #[test]
    fn manifest_parses_roles_and_logical_size() {
        let def = GuiDef::from_manifest(SAMPLE).unwrap();
        assert_eq!(def.kind, GuiKind::Chest);
        assert_eq!(def.role_slots(Role::Storage).len(), 2);
        assert_eq!(def.role_slots(Role::Hotbar).len(), 1);
        assert_eq!(def.role_slots(Role::PlayerInv).len(), 1);
        // 352x332 baked at scale 2 -> 176x166 logical.
        assert_eq!((def.logical_w, def.logical_h), (176.0, 166.0));
    }

    #[test]
    fn hover_block_converts_margin_to_base_px() {
        let def = GuiDef::from_manifest(SAMPLE).unwrap();
        let h = def.hover().unwrap();
        assert_eq!(h.margin, 4); // 8 canvas px / scale 2 = 4 base px
        assert_eq!(h.opacity, 0.5);
        assert_eq!(h.image, "chest_hover.png");
    }

    #[test]
    fn manifest_without_hover_has_none() {
        let json = r#"{"type":"chest","canvas":{"w":176,"h":166},"scale":1,"image":"chest.png","slots":[]}"#;
        assert!(GuiDef::from_manifest(json).unwrap().hover().is_none());
    }

    #[test]
    fn storage_and_inventory_round_trip_through_hit_test() {
        let def = GuiDef::from_manifest(SAMPLE).unwrap();
        let screen = (1280, 720);
        for i in 0..def.role_slots(Role::Storage).len() {
            let r = def.storage_rect(i, screen).unwrap();
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert_eq!(def.role_at(Role::Storage, screen, c), Some(i));
            // The hovered-slot lookup finds the same rect.
            assert_eq!(def.hovered_slot_rect(screen, c), Some(r));
        }
        // Inventory index 0 = hotbar slot; 9 = first main-grid slot.
        let hb = def.inventory_rect(0, screen).unwrap();
        assert_eq!(def.inventory_at(screen, (hb.x + 1.0, hb.y + 1.0)), Some(0));
        let main = def.inventory_rect(HOTBAR_LEN, screen).unwrap();
        assert_eq!(def.inventory_at(screen, (main.x + 1.0, main.y + 1.0)), Some(HOTBAR_LEN));
    }

    #[test]
    fn malformed_manifest_falls_back_to_none() {
        assert!(GuiDef::from_manifest("not json").is_none());
    }
}
