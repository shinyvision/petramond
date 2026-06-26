//! UI / HUD geometry: builds the hotbar, open-inventory panel, per-slot item
//! icons, stack-count digits and the drag cursor each frame from the renderer's
//! [`UiSnapshot`] into a reusable dynamic vertex buffer of [`UiVertex`]s.
//!
//! ALL layout math (GUI scale, slot rectangles, panel placement) lives in this
//! one module (split into per-screen submodules) so the renderer and the
//! [`slot_at_cursor`] hit-test used by the App (contract §9) can never diverge.
//! Coordinates are computed in **physical pixels** (origin top-left, y down) and
//! converted to NDC only when emitting vertices, so the pixel-space slot rects are
//! the shared source of truth. Each screen's layout, draw and hit-test live in one
//! submodule reading the SAME [`SlotRect`] functions so render and hit-test agree:
//! - [`hotbar`]: the closed HUD widget + selection highlight.
//! - [`inventory`]: the open 36-slot panel ([`slot_at_cursor`], [`cursor_in_panel`]).
//! - [`crafting`]: the 2×2 / 3×3 craft grid + result ([`craft_slot_at_cursor`]).
//! - [`furnace`]: the furnace slots + smelt/burn gauges ([`furnace_slot_at_cursor`]).
//! - [`chest`]: the 27-slot storage grid ([`chest_slot_at_cursor`]).
//! - [`icon`]: per-slot item icon projection + stack-count digits.
//!
//! Three kinds of geometry come out of [`build_ui`]:
//! - **gui quads** ([`UiBuild::verts`]): textured GUI sprites + solid-color fills,
//!   drawn by `ui_pipe` with the gui atlas. Emitted back-to-front (dim background,
//!   panel/hotbar, selection) so later quads paint over earlier ones.
//! - **icon quads** ([`UiBuild::icon_quads`]): one `(item, slot rect)` per filled
//!   slot. Each item's icon is rendered ONCE at renderer init into a cell of an
//!   icon-atlas texture (`render::renderer::icon_atlas`); the renderer resolves
//!   each entry to its cell and draws a single textured quad via `ui_pipe` with the
//!   icon atlas — no per-frame 3D geometry. These paint over the gui background.
//! - **overlay quads** ([`UiBuild::overlay_verts`]): the normal stack-count digits,
//!   drawn over the background and slot icons so they read on top.
//! - **drag quads** ([`UiBuild::drag_icon_quads`] + [`UiBuild::drag_overlay_verts`]):
//!   the cursor-held stack, drawn after the normal overlay so both its icon and count
//!   are always front-most.

mod chest;
mod crafting;
mod furnace;
mod hotbar;
// `pub(crate)` so the one-time icon-atlas bake (`renderer::icon_atlas`) can reach
// the `pub(crate)` MVP projection fns; the per-slot helpers stay `pub(super)`.
pub(crate) mod icon;
mod inventory;

use super::renderer::UiSnapshot;
use super::resources::GuiSprite;
use crate::item::ItemType;

// Slot-count constants the per-screen submodules reach via `super::`.
pub(super) use crate::inventory::{HOTBAR_LEN, TOTAL_SLOTS};

// --- App-facing hit-test re-exports (paths unchanged; consumed via `render::mod`
// by `app.rs` + the app tests). Each lives in its screen's submodule. ---
pub use chest::chest_slot_at_cursor;
pub use crafting::{craft_slot_at_cursor, CraftHit, CraftKind};
pub use furnace::{furnace_slot_at_cursor, FurnaceHit};
pub use inventory::{cursor_in_panel, slot_at_cursor};

/// A single UI vertex: NDC position (y up) + gui-atlas uv + RGBA color. `uv.x < 0`
/// is the solid-color sentinel (see `ui.wgsl`). 32 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// uv sentinel marking a solid-color quad (no atlas sample).
pub(super) const SOLID_UV: [f32; 2] = [-1.0, -1.0];

// --- Shared layout constants (physical pixels at GUI scale 1). ---

/// Slot interior side (px) — the textured icon area. Shared by every screen.
pub(super) const SLOT_PX: f32 = 16.0;

/// Classic inventory panel size (px): the 176×166 art sits in the top-left of the
/// 256×256 sheet. Shared by every OPEN screen (inventory / crafting / furnace /
/// chest all draw on a 176×166 panel) for the centred [`panel_origin`].
pub(super) const PANEL_W: f32 = 176.0;
pub(super) const PANEL_H: f32 = 166.0;

/// Open-inventory slot pitch (px): the classic `inventory.png` panel (176×166)
/// lays its slots at an 18px pitch (16px interior + 1px border each side). Used by
/// the open inventory grid, the craft grid and the chest grid — NOT the closed
/// hotbar widget (which uses its own 20px pitch in [`hotbar`]).
pub(super) const PANEL_PITCH: f32 = 18.0;
const SLOT_HOVER_FILL: [f32; 4] = [0.78, 0.98, 0.92, 0.13];
const SLOT_HOVER_EDGE: [f32; 4] = [0.72, 1.0, 0.94, 0.24];

/// One slot's pixel rectangle (interior, where the icon + digits go). All in
/// physical pixels, top-left origin, y down. The single source of truth shared
/// between every screen's draw and its hit-test.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlotRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl SlotRect {
    /// Whether physical-pixel point `(px, py)` lies within this slot's interior
    /// (half-open: includes the top-left edge, excludes the bottom-right). The one
    /// rectangle test every screen's hit-test shares, so a visible slot is always
    /// the slot that gets clicked.
    #[inline]
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// The CPU-built UI for this frame.
pub struct UiBuild {
    /// gui-atlas quads (sprites + solid fills), in paint order — the background
    /// drawn under the icons.
    pub verts: Vec<UiVertex>,
    /// One `(item, slot rect)` per filled slot this frame. Each item's icon was
    /// baked once into the icon atlas at renderer init; the renderer resolves each
    /// entry to its cell and emits a textured quad (no per-frame 3D geometry),
    /// painted over the gui background under the digits.
    pub icon_quads: Vec<(ItemType, SlotRect)>,
    /// gui-atlas quads drawn AFTER the normal icons (stack-count digits), so digits
    /// read on top of the icons.
    pub overlay_verts: Vec<UiVertex>,
    /// Cursor-held item icon quads, drawn after normal stack-count overlays.
    pub drag_icon_quads: Vec<(ItemType, SlotRect)>,
    /// Cursor-held stack-count digits, drawn after the cursor-held icon.
    pub drag_overlay_verts: Vec<UiVertex>,
}

/// Integer GUI scale chosen from the screen height (vanilla-style auto scale):
/// one scale step per ~240 logical px of height, clamped to `1..=4`. Returned as
/// `f32` since all layout multiplies by it.
pub fn gui_scale(screen: (u32, u32)) -> f32 {
    let (w, h) = screen;
    let by_h = (h / 240).max(1);
    let by_w = (w / 320).max(1);
    by_h.min(by_w).clamp(1, 4) as f32
}

/// The inventory panel's top-left pixel position (centred) for `screen` at
/// `scale`. The 176×166 art sits in the top-left of the 256×256 sheet. Shared by
/// every open screen (inventory / crafting / furnace / chest) since all draw on
/// the same centred 176×166 panel.
pub(super) fn panel_origin(screen: (u32, u32), scale: f32) -> (f32, f32) {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    let pw = PANEL_W * scale;
    let ph = PANEL_H * scale;
    ((w - pw) * 0.5, (h - ph) * 0.5)
}

/// Convert a physical-pixel point (top-left origin, y down) to NDC (y up). The one
/// pixel→clip conversion every UI submodule shares when emitting geometry.
#[inline]
pub(super) fn pixel_to_ndc(screen: (u32, u32), x: f32, y: f32) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0]
}

/// Push a textured gui-sprite quad covering pixel rect `(x,y,w,h)` with `color`.
pub(super) fn push_sprite(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    sprite: GuiSprite,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
) {
    let [u0, v0, u1, v1] = sprite.rect();
    push_quad_uv(out, screen, x, y, w, h, [u0, v0], [u1, v1], color);
}

/// Push a solid-color quad covering pixel rect `(x,y,w,h)`.
pub(super) fn push_solid(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
) {
    push_quad_uv(out, screen, x, y, w, h, SOLID_UV, SOLID_UV, color);
}

/// Push a quad covering pixel rect `(x,y,w,h)` with explicit uv corners (top-left
/// `uv_tl`, bottom-right `uv_br`). Two CCW triangles. y-down pixels → y-up NDC.
pub(super) fn push_quad_uv(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv_tl: [f32; 2],
    uv_br: [f32; 2],
    color: [f32; 4],
) {
    let p_tl = pixel_to_ndc(screen, x, y);
    let p_tr = pixel_to_ndc(screen, x + w, y);
    let p_br = pixel_to_ndc(screen, x + w, y + h);
    let p_bl = pixel_to_ndc(screen, x, y + h);
    let uv_tr = [uv_br[0], uv_tl[1]];
    let uv_bl = [uv_tl[0], uv_br[1]];
    let v = |pos: [f32; 2], uv: [f32; 2]| UiVertex { pos, uv, color };
    // tl, bl, br, tl, br, tr
    out.push(v(p_tl, uv_tl));
    out.push(v(p_bl, uv_bl));
    out.push(v(p_br, uv_br));
    out.push(v(p_tl, uv_tl));
    out.push(v(p_br, uv_br));
    out.push(v(p_tr, uv_tr));
}

fn push_slot_hover(out: &mut Vec<UiVertex>, screen: (u32, u32), r: SlotRect, scale: f32) {
    let b = scale.max(1.0).min(r.w * 0.25).min(r.h * 0.25);
    push_solid(out, screen, r.x, r.y, r.w, r.h, SLOT_HOVER_FILL);
    push_solid(out, screen, r.x, r.y, r.w, b, SLOT_HOVER_EDGE);
    push_solid(out, screen, r.x, r.y + r.h - b, r.w, b, SLOT_HOVER_EDGE);
    push_solid(out, screen, r.x, r.y, b, r.h, SLOT_HOVER_EDGE);
    push_solid(out, screen, r.x + r.w - b, r.y, b, r.h, SLOT_HOVER_EDGE);
}

fn hovered_slot_rect(ui: &UiSnapshot, screen: (u32, u32), scale: f32) -> Option<SlotRect> {
    // Slot hover is a GUI-only affordance: with no screen open the cursor is grabbed
    // for mouse-look and isn't pointing at slots, so the closed HUD hotbar must never
    // light up under the pointer (the active slot keeps its own selection box). Only an
    // open screen highlights a slot on hover.
    if !ui.open {
        return None;
    }
    let (px, py) = ui.cursor_px;
    if ui.furnace.is_some() {
        for slot in [FurnaceHit::Input, FurnaceHit::Fuel, FurnaceHit::Output] {
            let r = furnace::furnace_slot_rect(slot, screen, scale);
            if r.contains(px, py) {
                return Some(r);
            }
        }
    } else if ui.chest.is_some() {
        for i in 0..crate::chest::CHEST_SLOTS {
            if let Some(r) = chest::chest_slot_rect(i, screen, scale) {
                if r.contains(px, py) {
                    return Some(r);
                }
            }
        }
    } else {
        for i in 0..ui.panel.cols() * ui.panel.cols() {
            if let Some(r) = crafting::craft_slot_rect(ui.panel, i, screen, scale) {
                if r.contains(px, py) {
                    return Some(r);
                }
            }
        }
        let r = crafting::craft_result_rect(ui.panel, screen, scale);
        if r.contains(px, py) {
            return Some(r);
        }
    }

    // The 36 shared inventory slots in their open layout.
    (0..TOTAL_SLOTS)
        .filter_map(|i| inventory::slot_rect(i, screen, true, scale))
        .find(|r| r.contains(px, py))
}

/// Build the full UI for `ui` this frame, dispatching to each screen's submodule.
/// `verts`/`overlay_verts`/`icon_quads` are the caller-owned reusable buffers
/// (cleared, capacity retained). Paint order is back-to-front: screen background,
/// the shared inventory slot icons, the open screen's extra slots/gauges, then the
/// drag cursor on top — so later draws overpaint earlier ones.
pub fn build_ui(ui: &UiSnapshot, build: &mut UiBuild) {
    build.verts.clear();
    build.overlay_verts.clear();
    build.icon_quads.clear();
    build.drag_icon_quads.clear();
    build.drag_overlay_verts.clear();

    let screen = ui.screen;
    if screen.0 == 0 || screen.1 == 0 {
        return;
    }
    let scale = gui_scale(screen);

    // --- Background sprites (drawn first, under everything). ---
    if ui.open {
        // Dim the whole screen (~0.6 alpha black) behind the panel.
        push_solid(
            &mut build.verts,
            screen,
            0.0,
            0.0,
            screen.0 as f32,
            screen.1 as f32,
            [0.0, 0.0, 0.0, 0.6],
        );
        // The centred panel art for whichever open screen this is.
        let panel_sprite = if ui.chest.is_some() {
            GuiSprite::ChestPanel
        } else if ui.furnace.is_some() {
            GuiSprite::FurnacePanel
        } else {
            match ui.panel {
                CraftKind::Inventory => GuiSprite::InventoryPanel,
                CraftKind::Table => GuiSprite::CraftingTablePanel,
            }
        };
        let (ox, oy) = panel_origin(screen, scale);
        push_sprite(
            &mut build.verts,
            screen,
            panel_sprite,
            ox,
            oy,
            PANEL_W * scale,
            PANEL_H * scale,
            [1.0, 1.0, 1.0, 1.0],
        );
    } else {
        hotbar::build_background(ui, build, screen, scale);
    }

    if let Some(r) = hovered_slot_rect(ui, screen, scale) {
        push_slot_hover(&mut build.verts, screen, r, scale);
    }

    // --- Per-slot item icons + stack-count digits (the inventory slots, shared by
    // every screen: closed = the 9 hotbar slots, open = all 36). ---
    inventory::build_slots(ui, build, screen, scale);

    // --- The open screen's own extra slots / gauges, replacing the craft grid for
    // the furnace + chest screens. ---
    if ui.open {
        if ui.furnace.is_some() {
            furnace::build(ui, build, screen, scale);
        } else if ui.chest.is_some() {
            chest::build(ui, build, screen, scale);
        } else {
            crafting::build(ui, build, screen, scale);
        }
    }

    // --- Drag cursor: kept in its own final layer so icon + count stay in front. ---
    if ui.open {
        if let Some((item, count)) = ui.cursor {
            if item != ItemType::Air && count > 0 {
                let s = SLOT_PX * scale;
                let (cx, cy) = ui.cursor_px;
                // Center the icon on the cursor.
                let r = SlotRect {
                    x: cx - s * 0.5,
                    y: cy - s * 0.5,
                    w: s,
                    h: s,
                };
                build.drag_icon_quads.push((item, r));
                if count > 1 {
                    icon::push_count(
                        &mut build.drag_overlay_verts,
                        screen,
                        count as u32,
                        r,
                        scale,
                    );
                }
            }
        }
    }
}

/// The hotbar widget's top edge in NDC — pinned by the first-person hand clearance
/// test. Lives in [`hotbar`]; re-exported here so the public `ui::hotbar_top_ndc`
/// path stays unchanged (consumed by the hand clearance regression test).
#[cfg(test)]
#[allow(unused_imports)]
pub use hotbar::hotbar_top_ndc;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    pub(super) fn snap_open(open: bool) -> UiSnapshot {
        let mut s = UiSnapshot {
            open,
            panel: CraftKind::Inventory,
            screen: (1280, 720),
            cursor_px: (640.0, 360.0),
            active: 2,
            slots: [None; TOTAL_SLOTS],
            craft: [None; crate::crafting::MAX_CELLS],
            result: None,
            cursor: None,
            furnace: None,
            chest: None,
        };
        // A block-cube item and a sprite item in the hotbar.
        s.slots[0] = Some((ItemType::Stone, 64));
        s.slots[7] = Some((ItemType::Poppy, 1));
        s
    }

    /// An empty reusable build buffer for the tests.
    pub(super) fn empty_build() -> UiBuild {
        UiBuild {
            verts: Vec::new(),
            icon_quads: Vec::new(),
            overlay_verts: Vec::new(),
            drag_icon_quads: Vec::new(),
            drag_overlay_verts: Vec::new(),
        }
    }

    #[test]
    fn gui_scale_is_clamped_and_increases_with_height() {
        assert_eq!(gui_scale((320, 240)), 1.0);
        assert!(gui_scale((1920, 1080)) >= 2.0);
        // Tiny screens never go below 1.
        assert_eq!(gui_scale((10, 10)), 1.0);
        // Huge screens cap at 4.
        assert_eq!(gui_scale((10000, 10000)), 4.0);
    }

    #[test]
    fn open_build_dims_and_draws_panel() {
        let mut build = empty_build();
        let mut s = snap_open(true);
        s.cursor = Some((ItemType::Dirt, 12));
        build_ui(&s, &mut build);
        // Dim quad + panel sprite = at least 12 verts.
        assert!(build.verts.len() >= 12);
        // Two slot items in the normal layer + the drag cursor in the front layer.
        assert_eq!(build.icon_quads.len(), 2);
        assert_eq!(build.drag_icon_quads.len(), 1);
        assert!(!build.drag_overlay_verts.is_empty());
    }

    #[test]
    fn build_ui_reuses_icon_quads_without_growth() {
        // The icon-quad list is cleared + refilled each frame, never reallocated,
        // mirroring the per-frame no-allocation performance rule.
        let mut build = empty_build();
        build_ui(&snap_open(true), &mut build);
        let cap = build.icon_quads.capacity();
        assert!(cap > 0, "first build should record icon quads");
        // Rebuild with the closed (smaller) UI: cleared + refilled, capacity kept.
        build_ui(&snap_open(false), &mut build);
        assert_eq!(build.icon_quads.capacity(), cap, "icon-quad buffer reused");
    }

    #[test]
    fn empty_screen_builds_nothing() {
        let mut build = empty_build();
        let s = UiSnapshot {
            screen: (0, 0),
            ..snap_open(false)
        };
        build_ui(&s, &mut build);
        assert!(
            build.verts.is_empty()
                && build.icon_quads.is_empty()
                && build.drag_icon_quads.is_empty()
        );
    }
}
