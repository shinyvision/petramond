//! UI / HUD geometry: turns the renderer's [`UiSnapshot`] into the per-frame
//! vertex buffers for one GUI — the hotbar HUD, or an open inventory / crafting
//! table / furnace / chest panel.
//!
//! Every screen is DATA-DRIVEN: its panel art, slot positions, hover highlight and
//! dynamic overlays come from a baked manifest ([`crate::gui`]). There are no
//! per-screen layout modules — [`build_ui`] is generic over [`GuiKind`], reading
//! the open kind's [`GuiDef`] and the matching slice of game state from the
//! snapshot. The SAME def backs the App's click hit-test (`crate::gui::hit`), so
//! what's drawn and what's clicked can never diverge.
//!
//! All layout math is in **physical pixels** (origin top-left, y down) and
//! converted to NDC only when emitting vertices. [`build_ui`] fills, in paint
//! order back-to-front:
//! - `dim`: the fullscreen menu backdrop (solid; menus only).
//! - `panel`: the baked panel PNG quad (its own texture).
//! - `overlays`: dynamic [`OverlayTag`] quads clipped by game state (furnace
//!   gauges), each its own texture — `overlay_spans` records the per-overlay
//!   `(tag, vertex count)` so the renderer binds the right texture per quad.
//! - `hover`: the hover / selection highlight (its own texture), over the slot
//!   under the cursor — or, for the hotbar HUD, the active slot.
//! - `icon_quads`: one `(item, slot rect)` per filled slot (icon atlas).
//! - `counts`: stack-count digits (solid), over the icons.
//! - `drag_icon_quads` + `drag_counts`: the cursor-held stack, front-most.

// `pub(crate)` so the one-time icon-atlas bake (`renderer::icon_atlas`) can reach
// the `pub(crate)` MVP projection fns; the per-slot helpers stay `pub(super)`.
pub(crate) mod icon;

use crate::gui::UiSnapshot;
use crate::gui::{self as gui_layout, gui_scale, GuiKind, OverlayTag, Role, SlotRect};
use crate::inventory::HOTBAR_LEN;
use crate::item::ItemType;

/// A single UI vertex: NDC position (y up) + texture uv + RGBA color. `uv.x < 0`
/// is the solid-color sentinel (see `ui.wgsl`). 32 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// uv sentinel marking a solid-color quad (no texture sample).
pub(super) const SOLID_UV: [f32; 2] = [-1.0, -1.0];

/// Opaque white tint: draw a textured quad at full color.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Slot interior side (logical px) — the textured icon area. The drag cursor
/// sizes the held item to this; slot rects themselves come from the manifest.
pub(super) const SLOT_PX: f32 = 16.0;

/// Heart sprite side in logical px (the atlas cells are 9×9), scaled by `gui_scale`.
const HEART_PX: f32 = 9.0;
/// Horizontal step between hearts in logical px — one under the sprite width so
/// neighbours tuck together like the vanilla bar.
const HEART_STEP: f32 = 8.0;
/// Gap from the screen's left and bottom edges to the heart bar, logical px.
const HEART_MARGIN: f32 = 8.0;
/// One heart atlas cell as a fraction of atlas width — three cells: empty | half | full.
const HEART_CELL_U: f32 = 1.0 / 3.0;

/// One dynamic-overlay quad's place in [`UiBuild::overlays`]: which texture binds
/// it ([`OverlayTag`]) and how many vertices it spans (always 6 for one quad).
#[derive(Copy, Clone, Debug)]
pub struct OverlaySpan {
    pub tag: OverlayTag,
    pub count: u32,
}

/// The CPU-built UI for this frame, in paint order. Buffers are reused across
/// frames (cleared, capacity retained) per the no-per-frame-allocation rule.
#[derive(Default)]
pub struct UiBuild {
    /// Fullscreen dim backdrop behind an open menu (solid; empty for the HUD).
    pub dim: Vec<UiVertex>,
    /// The baked panel PNG quad (textured by its own panel bind group).
    pub panel: Vec<UiVertex>,
    /// Dynamic overlay quads (furnace gauges) concatenated; each its own texture.
    pub overlays: Vec<UiVertex>,
    /// Per-overlay `(tag, vertex count)` describing how to slice + bind `overlays`.
    pub overlay_spans: Vec<OverlaySpan>,
    /// Hover / selection highlight quad (its own texture). Empty when nothing is
    /// highlighted.
    pub hover: Vec<UiVertex>,
    /// HUD heart quads (bottom-left health bar), sampling the heart atlas. Empty for a
    /// spectator or behind an open menu.
    pub hearts: Vec<UiVertex>,
    /// One `(item, slot rect)` per filled slot. The renderer resolves each to its
    /// pre-baked icon-atlas cell and emits a textured quad.
    pub icon_quads: Vec<(ItemType, SlotRect)>,
    /// Like [`icon_quads`](Self::icon_quads) but drawn semi-transparent (greyed) — the
    /// furniture-workbench results the placed block can't yet make (not enough input).
    pub dim_icon_quads: Vec<(ItemType, SlotRect)>,
    /// Stack-count digits (solid), drawn over the icons.
    pub counts: Vec<UiVertex>,
    /// Cursor-held item icon, drawn front-most.
    pub drag_icon_quads: Vec<(ItemType, SlotRect)>,
    /// Cursor-held stack-count digits, drawn over the cursor icon.
    pub drag_counts: Vec<UiVertex>,
    /// Which baked GUI this frame draws, so the renderer binds the matching panel /
    /// hover / overlay textures. `None` when nothing is drawn.
    pub(crate) kind: Option<GuiKind>,
}

impl UiBuild {
    fn clear(&mut self) {
        self.dim.clear();
        self.panel.clear();
        self.overlays.clear();
        self.overlay_spans.clear();
        self.hover.clear();
        self.hearts.clear();
        self.icon_quads.clear();
        self.dim_icon_quads.clear();
        self.counts.clear();
        self.drag_icon_quads.clear();
        self.drag_counts.clear();
        self.kind = None;
    }
}

/// Convert a physical-pixel point (top-left origin, y down) to NDC (y up).
#[inline]
pub(super) fn pixel_to_ndc(screen: (u32, u32), x: f32, y: f32) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [x / w * 2.0 - 1.0, 1.0 - y / h * 2.0]
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

/// The game state filling slot `i` of `role`, as `(item, count)` — the one place
/// that maps a manifest role to its backing model. `None` for an empty slot or a
/// decorative role.
fn slot_item(ui: &UiSnapshot, role: Role, i: usize) -> Option<(ItemType, u32)> {
    let stack = |s: crate::item::ItemStack| (s.item, s.count as u32);
    match role {
        Role::Storage => ui
            .chest
            .and_then(|c| c.slots.get(i).copied().flatten())
            .map(stack),
        Role::Hotbar => ui
            .slots
            .get(i)
            .copied()
            .flatten()
            .map(|(it, c)| (it, c as u32)),
        Role::PlayerInv => ui
            .slots
            .get(HOTBAR_LEN + i)
            .copied()
            .flatten()
            .map(|(it, c)| (it, c as u32)),
        Role::CraftInput => ui
            .craft
            .get(i)
            .copied()
            .flatten()
            .map(|(it, c)| (it, c as u32)),
        Role::CraftResult => ui.result.map(|(it, c)| (it, c as u32)),
        Role::FurnaceInput => ui.furnace.and_then(|f| f.input).map(stack),
        Role::FurnaceFuel => ui.furnace.and_then(|f| f.fuel).map(stack),
        Role::FurnaceOutput => ui.furnace.and_then(|f| f.output).map(stack),
        Role::WorkbenchInput => ui.workbench.as_ref().and_then(|w| w.input).map(stack),
        // The result grid carries a craftable flag (greyed when not), so it's drawn
        // specially in `build_ui` rather than through this (item, count) mapping.
        Role::WorkbenchResult => None,
        Role::Generic | Role::Other => None,
    }
}

/// Emit a dynamic overlay clipped by `frac` (`0..=1`) of game progress, recording
/// its span so the renderer binds the overlay's own texture. No-op at `frac <= 0`.
/// The fill direction is the overlay's defining behaviour: the smelt arrow grows
/// left→right with cook progress; the burn flame depletes top→down (the bottom
/// `frac` stays lit) as fuel runs out.
fn push_overlay(
    build: &mut UiBuild,
    def: &gui_layout::GuiDef,
    screen: (u32, u32),
    tag: OverlayTag,
    frac: f32,
) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let Some(r) = def.overlay_rect(tag, screen) else {
        return;
    };
    let start = build.overlays.len();
    match tag {
        OverlayTag::FurnaceArrow => {
            push_quad_uv(
                &mut build.overlays,
                screen,
                r.x,
                r.y,
                r.w * frac,
                r.h,
                [0.0, 0.0],
                [frac, 1.0],
                WHITE,
            );
        }
        OverlayTag::FurnaceFlame => {
            let lit_h = r.h * frac;
            let top = r.h - lit_h;
            push_quad_uv(
                &mut build.overlays,
                screen,
                r.x,
                r.y + top,
                r.w,
                lit_h,
                [0.0, 1.0 - frac],
                [1.0, 1.0],
                WHITE,
            );
        }
        OverlayTag::Other => return,
    }
    let count = (build.overlays.len() - start) as u32;
    build.overlay_spans.push(OverlaySpan { tag, count });
}

/// Emit the bottom-left heart bar for `health` (half-heart points). Every heart gets an
/// empty container, then a full or half heart laid over it per the current health, so a
/// damaged heart shows the container through its missing portion — the vanilla read.
/// Scaled with the rest of the HUD. Called only for the [`GuiKind::Hotbar`] HUD.
fn push_hearts(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    health: crate::gui::HealthView,
    scale: f32,
) {
    let hearts = (health.max / 2).max(0);
    if hearts == 0 {
        return;
    }
    let size = HEART_PX * scale;
    let step = HEART_STEP * scale;
    let margin = HEART_MARGIN * scale;
    let y = screen.1 as f32 - margin - size;
    // Atlas cell `c` (0 empty, 1 half, 2 full) as top-left / bottom-right uv corners.
    let cell_uv = |c: i32| -> ([f32; 2], [f32; 2]) {
        let u0 = c as f32 * HEART_CELL_U;
        ([u0, 0.0], [u0 + HEART_CELL_U, 1.0])
    };
    let current = health.current.clamp(0, health.max);
    for i in 0..hearts {
        let x = margin + i as f32 * step;
        let (tl, br) = cell_uv(0);
        push_quad_uv(out, screen, x, y, size, size, tl, br, WHITE);
        // 2 points = full heart, 1 = half, 0 = just the empty container.
        let cell = match (current - i * 2).clamp(0, 2) {
            2 => 2,
            1 => 1,
            _ => continue,
        };
        let (tl, br) = cell_uv(cell);
        push_quad_uv(out, screen, x, y, size, size, tl, br, WHITE);
    }
}

/// Build the full UI for `ui` this frame from its open [`GuiKind`]'s baked def.
/// The buffers are the caller-owned reusable [`UiBuild`] (cleared, capacity kept).
pub fn build_ui(ui: &UiSnapshot, build: &mut UiBuild) {
    build.clear();

    let screen = ui.screen;
    if screen.0 == 0 || screen.1 == 0 {
        return;
    }
    let scale = gui_scale(screen);
    let kind = ui.kind;
    let Some(def) = gui_layout::def(kind) else {
        return; // No baked manifest for this screen => nothing to draw.
    };
    build.kind = Some(kind);

    // Dim the screen behind an open menu (the HUD hotbar has no backdrop).
    if ui.open {
        push_solid(
            &mut build.dim,
            screen,
            0.0,
            0.0,
            screen.0 as f32,
            screen.1 as f32,
            [0.0, 0.0, 0.0, 0.6],
        );
    }

    // Panel art.
    let pr = def.panel_rect(screen);
    push_quad_uv(
        &mut build.panel,
        screen,
        pr.x,
        pr.y,
        pr.w,
        pr.h,
        [0.0, 0.0],
        [1.0, 1.0],
        WHITE,
    );

    // Dynamic overlays (the furnace's smelt arrow + burn flame).
    if let Some(f) = ui.furnace {
        push_overlay(build, def, screen, OverlayTag::FurnaceArrow, f.cook01);
        push_overlay(build, def, screen, OverlayTag::FurnaceFlame, f.burn01);
    }

    // Hover / selection highlight. The hotbar HUD always highlights the active
    // slot (the held item); every open menu highlights the slot under the cursor.
    if let Some(h) = def.hover() {
        let highlighted = if kind == GuiKind::Hotbar {
            def.role_rect(Role::Hotbar, ui.active as usize, screen)
        } else {
            def.hovered_slot_rect(screen, ui.cursor_px)
        };
        if let Some(sr) = highlighted {
            let m = h.margin as f32 * scale;
            push_quad_uv(
                &mut build.hover,
                screen,
                sr.x - m,
                sr.y - m,
                sr.w + 2.0 * m,
                sr.h + 2.0 * m,
                [0.0, 0.0],
                [1.0, 1.0],
                [1.0, 1.0, 1.0, h.opacity],
            );
        }
    }

    // HUD hearts (bottom-left). Only on the hotbar HUD, so an open menu hides them —
    // and a spectator carries no health, so `None` draws nothing.
    if kind == GuiKind::Hotbar {
        if let Some(health) = ui.health {
            push_hearts(&mut build.hearts, screen, health, scale);
        }
    }

    // Every filled slot's item icon + stack count.
    def.for_each_slot(screen, |role, i, r| {
        // The workbench result grid is a list of offered recipes, each greyed when the
        // placed block isn't yet enough to craft it — drawn here (no stack count).
        if role == Role::WorkbenchResult {
            if let Some(&(item, craftable)) = ui.workbench.as_ref().and_then(|w| w.results.get(i)) {
                if item != ItemType::Air {
                    if craftable {
                        icon::push_slot_icon(build, screen, item, r);
                    } else {
                        icon::push_dim_slot_icon(build, screen, item, r);
                    }
                }
            }
            return;
        }
        let Some((item, count)) = slot_item(ui, role, i) else {
            return;
        };
        if item == ItemType::Air || count == 0 {
            return;
        }
        icon::push_slot_icon(build, screen, item, r);
        if count > 1 {
            icon::push_count(&mut build.counts, screen, count, r, scale);
        }
    });

    // Cursor-held stack (drag/drop), front-most — only with a menu open.
    if ui.open {
        if let Some((item, count)) = ui.cursor {
            if item != ItemType::Air && count > 0 {
                let s = SLOT_PX * scale;
                let (cx, cy) = ui.cursor_px;
                let r = SlotRect {
                    x: cx - s * 0.5,
                    y: cy - s * 0.5,
                    w: s,
                    h: s,
                };
                build.drag_icon_quads.push((item, r));
                if count > 1 {
                    icon::push_count(&mut build.drag_counts, screen, count as u32, r, scale);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(kind: GuiKind, open: bool) -> UiSnapshot {
        let mut s = UiSnapshot {
            kind,
            open,
            screen: (1280, 720),
            cursor_px: (640.0, 360.0),
            active: 2,
            ..Default::default()
        };
        s.slots[0] = Some((ItemType::Stone, 64));
        s
    }

    #[test]
    fn gui_scale_is_clamped_and_increases_with_height() {
        assert_eq!(gui_scale((320, 240)), 1.0);
        assert!(gui_scale((1920, 1080)) >= 2.0);
        assert_eq!(gui_scale((10, 10)), 1.0);
        assert_eq!(gui_scale((10000, 10000)), 4.0);
    }

    #[test]
    fn zero_screen_builds_nothing() {
        let mut b = UiBuild::default();
        let s = UiSnapshot {
            screen: (0, 0),
            ..snap(GuiKind::Hotbar, false)
        };
        build_ui(&s, &mut b);
        assert!(b.panel.is_empty() && b.icon_quads.is_empty() && b.kind.is_none());
    }

    #[test]
    fn hotbar_hud_draws_panel_active_selection_and_item() {
        // Uses the real baked hotbar manifest (the contract guard ensures it loads).
        let mut b = UiBuild::default();
        build_ui(&snap(GuiKind::Hotbar, false), &mut b);
        assert_eq!(b.kind, Some(GuiKind::Hotbar));
        assert!(!b.panel.is_empty(), "hotbar panel drawn");
        assert!(!b.hover.is_empty(), "active-slot selection drawn");
        assert!(b.dim.is_empty(), "HUD has no dim backdrop");
        assert_eq!(b.icon_quads.len(), 1, "the one hotbar item");
    }

    #[test]
    fn open_menu_dims_and_draws_drag_cursor() {
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Inventory, true);
        s.cursor = Some((ItemType::Dirt, 12));
        build_ui(&s, &mut b);
        assert!(!b.dim.is_empty(), "menu dims the screen");
        assert!(!b.panel.is_empty());
        assert_eq!(b.drag_icon_quads.len(), 1);
        assert!(!b.drag_counts.is_empty(), "drag count > 1 drawn");
    }

    #[test]
    fn hotbar_hud_draws_hearts_from_health() {
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Hotbar, false);
        s.health = Some(crate::gui::HealthView {
            current: 15,
            max: 20,
        });
        build_ui(&s, &mut b);
        assert!(!b.hearts.is_empty(), "the HUD draws the heart bar");
    }

    #[test]
    fn hearts_hidden_behind_a_menu_and_without_health() {
        // Behind an open inventory (kind != Hotbar) hearts are hidden even with health.
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Inventory, true);
        s.health = Some(crate::gui::HealthView {
            current: 20,
            max: 20,
        });
        build_ui(&s, &mut b);
        assert!(b.hearts.is_empty(), "no hearts behind an open menu");
        // On the HUD but with no health (a spectator): still nothing.
        let mut b2 = UiBuild::default();
        build_ui(&snap(GuiKind::Hotbar, false), &mut b2); // health defaults to None
        assert!(b2.hearts.is_empty(), "no hearts without survival health");
    }

    #[test]
    fn build_reuses_buffers_without_growth() {
        let mut b = UiBuild::default();
        build_ui(&snap(GuiKind::Inventory, true), &mut b);
        let cap = b.icon_quads.capacity();
        assert!(cap > 0);
        build_ui(&snap(GuiKind::Hotbar, false), &mut b);
        assert_eq!(b.icon_quads.capacity(), cap, "icon-quad buffer reused");
    }
}
