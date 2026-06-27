//! The data-driven GUI model: every screen (hotbar HUD, inventory, crafting
//! table, furnace, chest) is a baked PNG panel + a JSON manifest of typed slots,
//! authored in the `gui-builder` tool. This module owns the parsed model and ALL
//! of its layout math; both the renderer ([`super::ui::build_ui`]) and the App's
//! click hit-test read the SAME [`GuiDef`] so what's drawn and what's clicked can
//! never diverge.
//!
//! Baked GUIs are loaded from a directory at RUNTIME (not embedded) so re-baking
//! from the gui-builder + restarting picks them up with no recompile, and so the
//! optional hover / tagged-overlay PNGs can be present-or-not without a build
//! error. The dir is resolved at compile time relative to the crate root, so it
//! works regardless of the working directory.
//!
//! The model is generic over [`GuiKind`] and slot [`Role`]: a [`GuiDef`] holds
//! the logical panel size, per-role slot rects, an optional hover highlight, and
//! optional dynamic [`OverlayTag`] overlays (the furnace's smelt arrow / burn
//! flame). Manifest coordinates are baked-canvas pixels (authoring scale); they're
//! converted once to *logical* pixels so the game applies its own integer
//! `gui_scale` on top — every screen scales identically.
//!
//! ## The role→index contract
//! A manifest lists a role's slots in a stable order; the i-th slot of a role maps
//! to the i-th game slot of that role's domain (storage→chest slot i, player_inv→
//! inventory slot 9+i, hotbar→inventory slot i, craft_input→craft cell i). That
//! order MUST be row-major. [`GuiDef::validate`] enforces both the per-kind slot
//! counts and the row-major ordering at load, so a future re-bake can never
//! silently mis-route a click.

use super::gui_types::{CraftHit, FurnaceHit, WorkbenchHit};
use super::ui::{gui_scale, SlotRect};
use crate::game::MenuSlot;
use crate::inventory::HOTBAR_LEN;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where baked GUIs live. Absolute (baked at compile from the crate root) so the
/// game finds them no matter the CWD; the gui-builder bakes into this folder.
const BAKED_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/textures/gui/baked");

/// The closed-HUD hotbar's gap from the screen bottom (logical px), scaled by
/// `gui_scale`. Matches the classic 1px lift so the held-item hand clears it.
const HOTBAR_BOTTOM_MARGIN: f32 = 1.0;

static REGISTRY: OnceLock<Vec<Loaded>> = OnceLock::new();

/// One baked GUI: its parsed def plus the resolved on-disk paths the renderer
/// loads its panel / hover / overlay textures from.
struct Loaded {
    def: GuiDef,
    panel_path: PathBuf,
    hover_path: Option<PathBuf>,
    overlay_paths: Vec<(OverlayTag, PathBuf)>,
}

fn load_baked() -> Vec<Loaded> {
    let dir = Path::new(BAKED_DIR);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out; // no baked dir => no GUIs at all.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Some(def) = GuiDef::from_manifest(&text) else {
            eprintln!("gui: ignoring unparseable manifest {}", path.display());
            continue;
        };
        if let Err(e) = def.validate() {
            eprintln!("gui: ignoring {} — {e}", path.display());
            continue;
        }
        let panel_path = dir.join(&def.image);
        if !panel_path.exists() {
            eprintln!("gui: ignoring {} — missing panel art {}", path.display(), def.image);
            continue;
        }
        let hover_path = def.hover.as_ref().map(|h| dir.join(&h.image)).filter(|p| p.exists());
        let overlay_paths = def
            .overlays
            .iter()
            .filter_map(|o| {
                let p = dir.join(&o.image);
                p.exists().then_some((o.tag, p))
            })
            .collect();
        out.push(Loaded { def, panel_path, hover_path, overlay_paths });
    }
    out
}

fn registry() -> &'static [Loaded] {
    REGISTRY.get_or_init(load_baked)
}

/// The def for `kind`, or `None` if no (valid) manifest is baked for it.
pub(crate) fn def(kind: GuiKind) -> Option<&'static GuiDef> {
    registry().iter().find(|l| l.def.kind == kind).map(|l| &l.def)
}

/// (kind, panel PNG path) for every baked GUI — the renderer uploads each into a
/// texture + bind group keyed by [`GuiTexId::Panel`].
pub(crate) fn baked_panels() -> Vec<(GuiKind, PathBuf)> {
    registry().iter().map(|l| (l.def.kind, l.panel_path.clone())).collect()
}

/// (kind, hover PNG path) for every baked GUI that declares a hover highlight.
pub(crate) fn baked_hovers() -> Vec<(GuiKind, PathBuf)> {
    registry().iter().filter_map(|l| l.hover_path.clone().map(|p| (l.def.kind, p))).collect()
}

/// (kind, tag, overlay PNG path) for every baked dynamic overlay (furnace gauges).
pub(crate) fn baked_overlays() -> Vec<(GuiKind, OverlayTag, PathBuf)> {
    registry()
        .iter()
        .flat_map(|l| l.overlay_paths.iter().map(move |(t, p)| (l.def.kind, *t, p.clone())))
        .collect()
}

/// The slot under the cursor in `kind`'s layout, as a game [`MenuSlot`] — the one
/// hit-test the App routes through `ContainerMenu::click`. `None` if the cursor is
/// over no slot (or `kind` has no baked manifest).
pub(crate) fn hit(kind: GuiKind, screen: (u32, u32), cursor: (f32, f32)) -> Option<MenuSlot> {
    let (role, i) = def(kind)?.role_at_any(screen, cursor)?;
    role.menu_slot(i)
}

/// Whether the cursor lies over `kind`'s panel rect (used to decide whether an
/// off-slot click throws the held stack vs does nothing on the panel art).
pub(crate) fn panel_contains(kind: GuiKind, screen: (u32, u32), cursor: (f32, f32)) -> bool {
    def(kind).is_some_and(|d| d.panel_rect(screen).contains(cursor.0, cursor.1))
}

// ---- manifest JSON (the gui-builder bake format) --------------------------

/// Which container a baked GUI is for. Matches the gui-builder's `type` field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiKind {
    Chest,
    Inventory,
    CraftingTable,
    Furnace,
    Hotbar,
    FurnitureWorkbench,
    #[serde(other)]
    Other,
}

/// A slot's purpose. Matches the gui-builder's slot `role` field. Each role maps
/// to a concrete game [`MenuSlot`] via [`Role::menu_slot`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Generic,
    Storage,
    PlayerInv,
    Hotbar,
    CraftInput,
    CraftResult,
    FurnaceInput,
    FurnaceFuel,
    FurnaceOutput,
    WorkbenchInput,
    WorkbenchResult,
    #[serde(other)]
    Other,
}

impl Role {
    /// Map this role + its in-role index to the game slot a click resolves to.
    /// `None` for decorative roles (`Generic`/`Other`) that own no game slot, so a
    /// stray such slot in a bake can never route a click.
    pub(crate) fn menu_slot(self, i: usize) -> Option<MenuSlot> {
        Some(match self {
            Role::Storage => MenuSlot::Chest(i),
            Role::Hotbar => MenuSlot::Inventory(i),
            Role::PlayerInv => MenuSlot::Inventory(HOTBAR_LEN + i),
            Role::CraftInput => MenuSlot::Craft(CraftHit::Input(i)),
            Role::CraftResult => MenuSlot::Craft(CraftHit::Result),
            Role::FurnaceInput => MenuSlot::Furnace(FurnaceHit::Input),
            Role::FurnaceFuel => MenuSlot::Furnace(FurnaceHit::Fuel),
            Role::FurnaceOutput => MenuSlot::Furnace(FurnaceHit::Output),
            Role::WorkbenchInput => MenuSlot::Workbench(WorkbenchHit::Input),
            Role::WorkbenchResult => MenuSlot::Workbench(WorkbenchHit::Result(i)),
            Role::Generic | Role::Other => return None,
        })
    }
}

/// A dynamic overlay drawn over the panel and clipped at runtime by game state —
/// the furnace's smelt arrow (fills with cook progress) and burn flame (depletes
/// with remaining fuel). Matches the gui-builder's tagged-layer `tag` field.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OverlayTag {
    FurnaceArrow,
    FurnaceFlame,
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
    #[serde(default)]
    tagged: Vec<TaggedJson>,
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

#[derive(Deserialize)]
struct TaggedJson {
    tag: OverlayTag,
    image: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

fn default_opacity() -> f32 {
    1.0
}

/// The hover highlight for a GUI: the graphic + how far it extends beyond a slot.
pub(crate) struct HoverDef {
    /// Logical px the highlight extends beyond the slot on every side.
    pub margin: i32,
    pub opacity: f32,
    /// Panel-relative PNG filename (resolved to a path in [`load_baked`]).
    image: String,
}

/// A dynamic overlay's placement: its tag (how the game clips it) + logical rect.
struct OverlayDef {
    tag: OverlayTag,
    /// `[x, y, w, h]` in logical px (canvas px ÷ authoring scale).
    base: [f32; 4],
    /// Panel-relative PNG filename (resolved to a path in [`load_baked`]).
    image: String,
}

/// How a panel is anchored on screen. Menus centre; the hotbar HUD pins to the
/// bottom edge.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Anchor {
    Center,
    BottomCenter,
}

/// A parsed data-driven GUI: logical panel size (canvas ÷ authoring scale) plus
/// per-role slot rects in *logical* (base) pixels, ready to centre + scale by the
/// game's integer `gui_scale`.
pub(crate) struct GuiDef {
    kind: GuiKind,
    logical_w: f32,
    logical_h: f32,
    roles: HashMap<Role, Vec<[f32; 4]>>,
    overlays: Vec<OverlayDef>,
    /// Panel PNG filename (resolved to a path in [`load_baked`]).
    image: String,
    hover: Option<HoverDef>,
}

impl GuiDef {
    fn from_manifest(s: &str) -> Option<GuiDef> {
        let m: Manifest = serde_json::from_str(s).ok()?;
        let scale = m.scale.max(1) as f32;
        let logical = |x: i32| x as f32 / scale;
        // Manifest coords are baked-canvas pixels (authoring scale); convert to
        // logical/base pixels so the game applies its own gui_scale on top.
        let mut roles: HashMap<Role, Vec<[f32; 4]>> = HashMap::new();
        for s in &m.slots {
            roles
                .entry(s.role)
                .or_default()
                .push([logical(s.x), logical(s.y), logical(s.w), logical(s.h)]);
        }
        let overlays = m
            .tagged
            .into_iter()
            .map(|t| OverlayDef {
                tag: t.tag,
                base: [logical(t.x), logical(t.y), logical(t.w), logical(t.h)],
                image: t.image,
            })
            .collect();
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
            overlays,
            image: m.image,
            hover,
        })
    }

    /// Enforce the role→index contract: the expected per-role slot counts for this
    /// kind, and row-major ordering of every multi-slot role. A mismatch means a
    /// bad bake; the caller skips the manifest rather than silently mis-routing.
    fn validate(&self) -> Result<(), String> {
        let n = |r: Role| self.role_slots(r).len();
        let want: &[(Role, usize)] = match self.kind {
            GuiKind::Chest => &[(Role::Storage, 27), (Role::PlayerInv, 27), (Role::Hotbar, 9)],
            GuiKind::Inventory => {
                &[(Role::PlayerInv, 27), (Role::Hotbar, 9), (Role::CraftInput, 4), (Role::CraftResult, 1)]
            }
            GuiKind::CraftingTable => {
                &[(Role::PlayerInv, 27), (Role::Hotbar, 9), (Role::CraftInput, 9), (Role::CraftResult, 1)]
            }
            GuiKind::Furnace => &[
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
                (Role::FurnaceInput, 1),
                (Role::FurnaceFuel, 1),
                (Role::FurnaceOutput, 1),
            ],
            GuiKind::Hotbar => &[(Role::Hotbar, 9)],
            GuiKind::FurnitureWorkbench => &[
                (Role::PlayerInv, 27),
                (Role::Hotbar, 9),
                (Role::WorkbenchInput, 1),
                (Role::WorkbenchResult, 21),
            ],
            GuiKind::Other => return Err("unknown gui type".to_string()),
        };
        for &(role, count) in want {
            if n(role) != count {
                return Err(format!("{:?} wants {count} {role:?} slots, found {}", self.kind, n(role)));
            }
        }
        // Multi-slot roles must be row-major so in-role index == game slot index.
        for role in [
            Role::Storage,
            Role::PlayerInv,
            Role::Hotbar,
            Role::CraftInput,
            Role::WorkbenchResult,
        ] {
            check_row_major(role, self.role_slots(role))?;
        }
        Ok(())
    }

    fn anchor(&self) -> Anchor {
        match self.kind {
            GuiKind::Hotbar => Anchor::BottomCenter,
            _ => Anchor::Center,
        }
    }

    /// `(offset_x, offset_y, scale)` placing the panel on `screen`.
    fn placement(&self, screen: (u32, u32)) -> (f32, f32, f32) {
        let s = gui_scale(screen);
        let (w, h) = (screen.0 as f32, screen.1 as f32);
        let (lw, lh) = (self.logical_w * s, self.logical_h * s);
        let (ox, oy) = match self.anchor() {
            Anchor::Center => ((w - lw) * 0.5, (h - lh) * 0.5),
            Anchor::BottomCenter => ((w - lw) * 0.5, h - lh - HOTBAR_BOTTOM_MARGIN * s),
        };
        (ox, oy, s)
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

    /// Screen rect of slot `i` of `role`, or `None` if out of range.
    pub(crate) fn role_rect(&self, role: Role, i: usize, screen: (u32, u32)) -> Option<SlotRect> {
        self.role_slots(role).get(i).map(|b| self.rect(*b, screen))
    }

    /// Screen rect of the dynamic overlay `tag`, or `None` if this GUI has none.
    pub(crate) fn overlay_rect(&self, tag: OverlayTag, screen: (u32, u32)) -> Option<SlotRect> {
        self.overlays.iter().find(|o| o.tag == tag).map(|o| self.rect(o.base, screen))
    }

    /// Visit every slot of every role with its screen rect (for emitting icons).
    pub(crate) fn for_each_slot(&self, screen: (u32, u32), mut f: impl FnMut(Role, usize, SlotRect)) {
        for (&role, rects) in &self.roles {
            for (i, b) in rects.iter().enumerate() {
                f(role, i, self.rect(*b, screen));
            }
        }
    }

    /// The (role, in-role index) of the slot under the cursor, or `None`. Slots
    /// never overlap, so the first containing rect is unambiguous.
    pub(crate) fn role_at_any(&self, screen: (u32, u32), cursor: (f32, f32)) -> Option<(Role, usize)> {
        for (&role, rects) in &self.roles {
            for (i, b) in rects.iter().enumerate() {
                if self.rect(*b, screen).contains(cursor.0, cursor.1) {
                    return Some((role, i));
                }
            }
        }
        None
    }

    /// The screen rect of the slot under the cursor — used to place the hover
    /// highlight (inflated by the hover margin).
    pub(crate) fn hovered_slot_rect(&self, screen: (u32, u32), cursor: (f32, f32)) -> Option<SlotRect> {
        self.role_at_any(screen, cursor).and_then(|(role, i)| self.role_rect(role, i, screen))
    }
}

/// Verify a role's slot rects run top-to-bottom, left-to-right (row-major), so the
/// in-role index lines up with the game's row-major slot index.
fn check_row_major(role: Role, rects: &[[f32; 4]]) -> Result<(), String> {
    for w in rects.windows(2) {
        let (a, b) = (w[0], w[1]);
        // Same row when the y's are within half a slot height; then x must advance.
        let same_row = (a[1] - b[1]).abs() < a[3] * 0.5;
        let ok = if same_row { b[0] > a[0] } else { b[1] > a[1] };
        if !ok {
            return Err(format!("{role:?} slots are not in row-major order"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Inline samples so tests exercise the parser/geometry without pinning the
    // user's freely-re-baked manifests (their exact slot pixels are table data).
    const CHEST: &str = r#"{
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

    fn furnace_with_overlays() -> &'static str {
        r#"{
            "type": "furnace",
            "canvas": { "w": 352, "h": 332 },
            "scale": 2,
            "image": "furnace.png",
            "slots": [
                { "role": "furnace_input", "x": 162, "y": 25, "w": 28, "h": 28 }
            ],
            "tagged": [
                { "tag": "furnace_arrow", "image": "a.png", "x": 205, "y": 62, "w": 24, "h": 16 },
                { "tag": "furnace_flame", "image": "f.png", "x": 166, "y": 63, "w": 20, "h": 20 }
            ]
        }"#
    }

    #[test]
    fn manifest_parses_roles_and_logical_size() {
        let def = GuiDef::from_manifest(CHEST).unwrap();
        assert_eq!(def.kind, GuiKind::Chest);
        assert_eq!(def.role_slots(Role::Storage).len(), 2);
        assert_eq!(def.role_slots(Role::Hotbar).len(), 1);
        // 352x332 baked at scale 2 -> 176x166 logical.
        assert_eq!((def.logical_w, def.logical_h), (176.0, 166.0));
    }

    #[test]
    fn hover_block_converts_margin_to_logical_px() {
        let h = GuiDef::from_manifest(CHEST).unwrap().hover.unwrap();
        assert_eq!(h.margin, 4); // 8 canvas px / scale 2 = 4 logical px
        assert_eq!(h.opacity, 0.5);
        assert_eq!(h.image, "chest_hover.png");
    }

    #[test]
    fn tagged_overlays_parse_to_logical_rects() {
        let def = GuiDef::from_manifest(furnace_with_overlays()).unwrap();
        let screen = (1280, 720);
        // furnace_arrow (205,62,24,16) at scale 2 -> logical (102.5,31,12,8).
        let r = def.overlay_rect(OverlayTag::FurnaceArrow, screen).unwrap();
        let (ox, oy, s) = def.placement(screen);
        assert!((r.x - (ox + 102.5 * s)).abs() < 0.01);
        assert!((r.y - (oy + 31.0 * s)).abs() < 0.01);
        assert!((r.w - 12.0 * s).abs() < 0.01);
        assert!(def.overlay_rect(OverlayTag::FurnaceFlame, screen).is_some());
    }

    #[test]
    fn role_maps_to_the_right_menu_slot() {
        assert_eq!(Role::Storage.menu_slot(5), Some(MenuSlot::Chest(5)));
        assert_eq!(Role::Hotbar.menu_slot(3), Some(MenuSlot::Inventory(3)));
        assert_eq!(Role::PlayerInv.menu_slot(0), Some(MenuSlot::Inventory(HOTBAR_LEN)));
        assert_eq!(Role::CraftInput.menu_slot(4), Some(MenuSlot::Craft(CraftHit::Input(4))));
        assert_eq!(Role::CraftResult.menu_slot(0), Some(MenuSlot::Craft(CraftHit::Result)));
        assert_eq!(Role::FurnaceFuel.menu_slot(0), Some(MenuSlot::Furnace(FurnaceHit::Fuel)));
        assert_eq!(Role::Generic.menu_slot(0), None);
    }

    #[test]
    fn hit_round_trips_through_slot_center() {
        let def = GuiDef::from_manifest(CHEST).unwrap();
        let screen = (1280, 720);
        let r = def.role_rect(Role::Storage, 1, screen).unwrap();
        let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
        assert_eq!(def.role_at_any(screen, c), Some((Role::Storage, 1)));
        assert_eq!(def.hovered_slot_rect(screen, c), Some(r));
    }

    #[test]
    fn validate_rejects_wrong_slot_count() {
        // CHEST sample has only 2 storage slots, not the required 27.
        let def = GuiDef::from_manifest(CHEST).unwrap();
        assert!(def.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_row_major_storage() {
        let json = r#"{
            "type": "chest", "canvas": { "w": 352, "h": 332 }, "scale": 2, "image": "c.png",
            "slots": [
                { "role": "storage", "x": 52, "y": 26, "w": 32, "h": 32 },
                { "role": "storage", "x": 16, "y": 26, "w": 32, "h": 32 }
            ]
        }"#;
        let def = GuiDef::from_manifest(json).unwrap();
        assert!(check_row_major(Role::Storage, def.role_slots(Role::Storage)).is_err());
    }

    #[test]
    fn malformed_manifest_is_none() {
        assert!(GuiDef::from_manifest("not json").is_none());
    }

    #[test]
    fn baked_manifests_on_disk_all_validate() {
        // The real bakes the game ships must satisfy the role→index contract.
        // (Reads the actual assets dir; this is the contract guard, not table data.)
        for kind in [
            GuiKind::Chest,
            GuiKind::Inventory,
            GuiKind::CraftingTable,
            GuiKind::Furnace,
            GuiKind::Hotbar,
            GuiKind::FurnitureWorkbench,
        ] {
            assert!(def(kind).is_some(), "{kind:?} manifest missing or invalid");
        }
    }
}
