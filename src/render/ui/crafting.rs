//! The crafting grid: the 2×2 grid built into the inventory panel or the 3×3 grid
//! at a crafting table, plus the result preview. Owns the [`CraftKind`] layout
//! selector, the input/result slot rects (the single source of truth shared by the
//! renderer and the App's [`craft_slot_at_cursor`] hit-test) and the grid draw.

use super::super::renderer::UiSnapshot;
use super::{gui_scale, icon, panel_origin, SlotRect, UiBuild, PANEL_PITCH, SLOT_PX};
use crate::item::ItemType;

/// Which crafting layout the open panel shows: the 2×2 grid built into the
/// inventory panel, or the 3×3 grid at a crafting table. Drives both the craft
/// slot positions and which panel art is drawn.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CraftKind {
    Inventory,
    Table,
}

impl CraftKind {
    /// Grid side length: 2 (inventory) or 3 (table).
    #[inline]
    pub fn cols(self) -> usize {
        match self {
            CraftKind::Inventory => 2,
            CraftKind::Table => 3,
        }
    }

    /// Panel-relative (x, y) of the top-left input cell's interior (px), matching
    /// the vanilla `inventory.png` / `crafting_table.png` art.
    #[inline]
    fn grid_origin(self) -> (f32, f32) {
        match self {
            CraftKind::Inventory => (98.0, 18.0),
            CraftKind::Table => (30.0, 17.0),
        }
    }

    /// Panel-relative (x, y) of the result slot's interior (px).
    #[inline]
    fn result_origin(self) -> (f32, f32) {
        match self {
            CraftKind::Inventory => (154.0, 28.0),
            CraftKind::Table => (124.0, 35.0),
        }
    }
}

/// A hit-tested crafting slot: an input cell index (`0..cols*cols`, row-major) or
/// the output result slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CraftHit {
    Input(usize),
    Result,
}

/// Interior pixel rect of crafting input cell `i` (`0..cols*cols`), or `None` if
/// out of range. Uses the same panel placement + 18px pitch as the inventory.
pub fn craft_slot_rect(
    kind: CraftKind,
    i: usize,
    screen: (u32, u32),
    scale: f32,
) -> Option<SlotRect> {
    let cols = kind.cols();
    if i >= cols * cols {
        return None;
    }
    let (ox, oy) = panel_origin(screen, scale);
    let (gx, gy) = kind.grid_origin();
    let col = (i % cols) as f32;
    let row = (i / cols) as f32;
    Some(SlotRect {
        x: ox + (gx + col * PANEL_PITCH) * scale,
        y: oy + (gy + row * PANEL_PITCH) * scale,
        w: SLOT_PX * scale,
        h: SLOT_PX * scale,
    })
}

/// Interior pixel rect of the crafting result slot.
pub fn craft_result_rect(kind: CraftKind, screen: (u32, u32), scale: f32) -> SlotRect {
    let (ox, oy) = panel_origin(screen, scale);
    let (rx, ry) = kind.result_origin();
    SlotRect {
        x: ox + rx * scale,
        y: oy + ry * scale,
        w: SLOT_PX * scale,
        h: SLOT_PX * scale,
    }
}

/// The crafting slot under the cursor (an input cell or the result), or `None`.
/// Shared with the App so a visible craft slot is always the slot that's clicked.
pub fn craft_slot_at_cursor(
    kind: CraftKind,
    screen: (u32, u32),
    cursor_px: (f32, f32),
) -> Option<CraftHit> {
    let scale = gui_scale(screen);
    let (px, py) = cursor_px;
    for i in 0..kind.cols() * kind.cols() {
        if let Some(r) = craft_slot_rect(kind, i, screen, scale) {
            if r.contains(px, py) {
                return Some(CraftHit::Input(i));
            }
        }
    }
    craft_result_rect(kind, screen, scale)
        .contains(px, py)
        .then_some(CraftHit::Result)
}

/// Draw the crafting grid input cells + result preview (open crafting panels only;
/// the furnace / chest screens replace the grid with their own slots). Called by
/// [`super::build_ui`] for the crafting screens.
pub(super) fn build(ui: &UiSnapshot, build: &mut UiBuild, screen: (u32, u32), scale: f32) {
    let kind = ui.panel;
    for i in 0..kind.cols() * kind.cols() {
        let Some((item, count)) = ui.craft[i] else {
            continue;
        };
        if item == ItemType::Air || count == 0 {
            continue;
        }
        let Some(r) = craft_slot_rect(kind, i, screen, scale) else {
            continue;
        };
        icon::push_slot_icon(build, screen, item, r);
        if count > 1 {
            icon::push_count(&mut build.overlay_verts, screen, count as u32, r, scale);
        }
    }
    if let Some((item, count)) = ui.result {
        if item != ItemType::Air && count > 0 {
            let r = craft_result_rect(kind, screen, scale);
            icon::push_slot_icon(build, screen, item, r);
            if count > 1 {
                icon::push_count(&mut build.overlay_verts, screen, count as u32, r, scale);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::inventory::{cursor_in_panel, slot_at_cursor};
    use super::super::tests::{empty_build, snap_open};
    use super::*;

    #[test]
    fn open_build_draws_craft_cells_and_result() {
        let mut build = empty_build();
        let mut s = snap_open(true);
        s.panel = CraftKind::Table;
        // Two filled input cells + a result preview, no drag cursor.
        s.craft[0] = Some((ItemType::OakPlanks, 1));
        s.craft[4] = Some((ItemType::Stick, 3));
        s.result = Some((ItemType::WoodenPickaxe, 1));
        super::super::build_ui(&s, &mut build);
        // 2 hotbar items + 2 craft cells + 1 result = 5 icon quads.
        assert_eq!(build.icon_quads.len(), 5);
        // The two craft cells + result land within the panel and never overlap an
        // inventory slot rect (craft sits in the panel's top band).
        let scale = gui_scale(s.screen);
        for i in [0usize, 4] {
            let r = craft_slot_rect(CraftKind::Table, i, s.screen, scale).unwrap();
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert!(cursor_in_panel(s.screen, c), "craft cell {i} inside panel");
            assert_eq!(
                slot_at_cursor(s.screen, true, c),
                None,
                "craft cell {i} not an inv slot"
            );
            assert_eq!(
                craft_slot_at_cursor(CraftKind::Table, s.screen, c),
                Some(CraftHit::Input(i))
            );
        }
        let rr = craft_result_rect(CraftKind::Table, s.screen, scale);
        let rc = (rr.x + rr.w * 0.5, rr.y + rr.h * 0.5);
        assert_eq!(
            craft_slot_at_cursor(CraftKind::Table, s.screen, rc),
            Some(CraftHit::Result)
        );
    }
}
