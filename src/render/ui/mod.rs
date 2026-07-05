//! UI / HUD geometry: turns the renderer's [`UiSnapshot`] into the per-frame
//! vertex buffers for the GAME-OWNED content of a document-drawn screen.
//!
//! Every screen's chrome (panels, slot faces, hover, gauges, text, dim) is
//! drawn by the GUI-document runtime (`llama-ui` draw list → `renderer::doc_ui`).
//! [`build_ui`] emits only what the game owns on top of that: item icons +
//! stack counts in the document's solved slot cells, greyed workbench results,
//! the HUD hearts, and the cursor-held drag stack. When no document backs the
//! frame (`doc_slots` is `None`) nothing draws.
//!
//! All layout math is in **physical pixels** (origin top-left, y down) and
//! converted to NDC only when emitting vertices.

// `pub(crate)` so the one-time icon-atlas bake (`renderer::icon_atlas`) can reach
// the `pub(crate)` MVP projection fns; the per-slot helpers stay `pub(super)`.
pub(crate) mod icon;

use crate::gui::{gui_scale, GuiKind, Role, SlotRect, UiSnapshot};
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
/// sizes the held item to this; slot rects themselves come from the document.
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

/// The CPU-built UI for this frame, in paint order. Buffers are reused across
/// frames (cleared, capacity retained) per the no-per-frame-allocation rule.
#[derive(Default)]
pub struct UiBuild {
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
    /// The hurt-flash edge vignette (solid, per-vertex alpha gradient), drawn
    /// under everything else in the UI pass. Empty on a calm frame.
    pub vignette: Vec<UiVertex>,
}

impl UiBuild {
    fn clear(&mut self) {
        self.hearts.clear();
        self.icon_quads.clear();
        self.dim_icon_quads.clear();
        self.counts.clear();
        self.drag_icon_quads.clear();
        self.drag_counts.clear();
        self.vignette.clear();
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

/// The game-owned content of a document-drawn screen: item icons + counts in
/// the document's slot cells (inset one logical px so icons sit inside the
/// themed cell border), greyed workbench results, HUD hearts, and the
/// cursor-held drag stack. All chrome (panels, slot faces, hover, gauges,
/// text) is in the document draw list.
fn push_doc_game_content(
    ui: &UiSnapshot,
    build: &mut UiBuild,
    slots: &[crate::gui::DocSlot],
    scale: f32,
) {
    let screen = ui.screen;
    for slot in slots {
        let inset = scale;
        let r = SlotRect {
            x: slot.rect.x + inset,
            y: slot.rect.y + inset,
            w: (slot.rect.w - 2.0 * inset).max(0.0),
            h: (slot.rect.h - 2.0 * inset).max(0.0),
        };
        let i = slot.index as usize;
        if slot.role == Role::WorkbenchResult {
            if let Some(&(item, craftable)) = ui.workbench.as_ref().and_then(|w| w.results.get(i))
            {
                if item != ItemType::Air {
                    if craftable {
                        icon::push_slot_icon(build, screen, item, r);
                    } else {
                        icon::push_dim_slot_icon(build, screen, item, r);
                    }
                }
            }
            continue;
        }
        let Some((item, count)) = slot_item(ui, slot.role, i) else {
            continue;
        };
        if item == ItemType::Air || count == 0 {
            continue;
        }
        icon::push_slot_icon(build, screen, item, r);
        if count > 1 {
            icon::push_count(&mut build.counts, screen, count, r, scale);
        }
    }

    // HUD hearts only under the hotbar HUD, never behind menus/shell screens.
    if !ui.open && ui.kind == GuiKind::Hotbar {
        if let Some(health) = ui.health {
            push_hearts(&mut build.hearts, screen, health, scale);
        }
    }

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

/// The game state filling slot `i` of `role`, as `(item, count)` — the one place
/// that maps a slot role to its backing model. `None` for an empty slot or a
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
        // specially in `push_doc_game_content` rather than through this mapping.
        Role::WorkbenchResult => None,
        Role::Generic | Role::Other => None,
    }
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

/// Build the game-owned UI content for `ui` this frame. The buffers are the
/// caller-owned reusable [`UiBuild`] (cleared, capacity kept). Draws nothing
/// unless a GUI document solved this frame's slot cells (`doc_slots`).
pub fn build_ui(ui: &UiSnapshot, build: &mut UiBuild) {
    build.clear();

    let screen = ui.screen;
    if screen.0 == 0 || screen.1 == 0 {
        return;
    }
    if ui.hurt_flash > 0.0 {
        push_hurt_vignette(&mut build.vignette, screen, ui.hurt_flash);
    }
    let scale = gui_scale(screen);
    if let Some(doc_slots) = ui.doc_slots.clone() {
        push_doc_game_content(ui, build, &doc_slots, scale);
    }
}

/// Peak vignette alpha at full strength (reached toward the screen corners —
/// the shader's radial falloff scales it down toward the centre).
const VIGNETTE_MAX_ALPHA: f32 = 0.55;
const VIGNETTE_RED: [f32; 3] = [0.75, 0.03, 0.03];

/// uv sentinel marking the radial hurt vignette (see `ui.wgsl`): the fragment
/// stage computes a smooth falloff from the screen centre, so the overlay is
/// one connected ring rather than assembled bands.
const VIGNETTE_UV: [f32; 2] = [-2.0, -2.0];

/// The hurt flash: one fullscreen quad carrying the vignette sentinel; the
/// shader shapes it into a red radial rim (transparent centre, strongest at
/// the corners), scaled by `strength`.
fn push_hurt_vignette(out: &mut Vec<UiVertex>, screen: (u32, u32), strength: f32) {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    let a = VIGNETTE_MAX_ALPHA * strength.clamp(0.0, 1.0);
    push_quad_uv(
        out,
        screen,
        0.0,
        0.0,
        w,
        h,
        VIGNETTE_UV,
        VIGNETTE_UV,
        [VIGNETTE_RED[0], VIGNETTE_RED[1], VIGNETTE_RED[2], a],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::DocSlot;
    use std::sync::Arc;

    fn cell(role: Role, index: u32) -> DocSlot {
        DocSlot::new(
            role,
            index,
            SlotRect {
                x: 100.0 + index as f32 * 40.0,
                y: 100.0,
                w: 36.0,
                h: 36.0,
            },
        )
    }

    fn snap(kind: GuiKind, open: bool, slots: Vec<DocSlot>) -> UiSnapshot {
        let mut s = UiSnapshot {
            kind,
            open,
            screen: (1280, 720),
            cursor_px: (640.0, 360.0),
            active: 2,
            doc_slots: Some(Arc::new(slots)),
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
    fn zero_screen_and_missing_document_build_nothing() {
        let mut b = UiBuild::default();
        let s = UiSnapshot {
            screen: (0, 0),
            ..snap(GuiKind::Hotbar, false, vec![cell(Role::Hotbar, 0)])
        };
        build_ui(&s, &mut b);
        assert!(b.icon_quads.is_empty() && b.hearts.is_empty());

        // No doc_slots (no document drew the frame): the game draws nothing.
        let mut s = snap(GuiKind::Inventory, true, Vec::new());
        s.doc_slots = None;
        s.cursor = Some((ItemType::Dirt, 12));
        build_ui(&s, &mut b);
        assert!(b.icon_quads.is_empty() && b.drag_icon_quads.is_empty() && b.counts.is_empty());
    }

    #[test]
    fn doc_slots_emit_icons_counts_and_the_drag_stack() {
        let mut b = UiBuild::default();
        let mut s = snap(
            GuiKind::Inventory,
            true,
            vec![cell(Role::Hotbar, 0), cell(Role::Hotbar, 1)],
        );
        s.cursor = Some((ItemType::Dirt, 12));
        build_ui(&s, &mut b);
        assert_eq!(b.icon_quads.len(), 1, "only the filled cell draws an icon");
        let (item, r) = b.icon_quads[0];
        assert_eq!(item, ItemType::Stone);
        let outer = cell(Role::Hotbar, 0).rect;
        assert!(
            r.x > outer.x && r.y > outer.y && r.w < outer.w,
            "icon is inset inside the themed cell"
        );
        assert!(!b.counts.is_empty(), "stack count 64 drawn");
        assert_eq!(b.drag_icon_quads.len(), 1, "cursor-held stack drawn");
        assert!(!b.drag_counts.is_empty(), "drag count > 1 drawn");
    }

    #[test]
    fn hearts_only_on_the_hotbar_hud_with_health() {
        let health = Some(crate::gui::HealthView {
            current: 15,
            max: 20,
        });
        // HUD (kind Hotbar, no open menu) with health: hearts drawn.
        let mut b = UiBuild::default();
        let mut s = snap(GuiKind::Hotbar, false, vec![cell(Role::Hotbar, 0)]);
        s.health = health;
        build_ui(&s, &mut b);
        assert!(!b.hearts.is_empty(), "the HUD draws the heart bar");

        // Behind an open menu (kind != Hotbar): hidden even with health.
        let mut s = snap(GuiKind::Inventory, true, vec![cell(Role::Hotbar, 0)]);
        s.health = health;
        build_ui(&s, &mut b);
        assert!(b.hearts.is_empty(), "no hearts behind an open menu");

        // On the HUD but with no health (a spectator): still nothing.
        build_ui(
            &snap(GuiKind::Hotbar, false, vec![cell(Role::Hotbar, 0)]),
            &mut b,
        );
        assert!(b.hearts.is_empty(), "no hearts without survival health");
    }

    #[test]
    fn build_reuses_buffers_without_growth() {
        let mut b = UiBuild::default();
        let slots = vec![cell(Role::Hotbar, 0)];
        build_ui(&snap(GuiKind::Inventory, true, slots.clone()), &mut b);
        let cap = b.icon_quads.capacity();
        assert!(cap > 0);
        build_ui(&snap(GuiKind::Hotbar, false, slots), &mut b);
        assert_eq!(b.icon_quads.capacity(), cap, "icon-quad buffer reused");
    }
}
