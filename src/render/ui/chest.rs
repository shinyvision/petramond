//! The chest screen: the 3×9 storage grid above the reused player inventory + the
//! hotbar row. Owns the storage-grid layout (the single source of truth shared by
//! the renderer and the App's [`chest_slot_at_cursor`] hit-test) and the draw.

use super::super::renderer::UiSnapshot;
use super::{gui_scale, icon, panel_origin, SlotRect, UiBuild, PANEL_PITCH, SLOT_PX};
use crate::item::ItemType;

/// Chest storage grid origin (px), relative to the 176×166 panel's top-left: a 3×9
/// grid at (8, 18), matching the vanilla single-container art (`chest.png`). The
/// player inventory + hotbar below reuse the shared inventory slot positions.
const CHEST_GRID: (f32, f32) = (8.0, 18.0);

/// Interior pixel rect of chest storage slot `i` (`0..27`, row-major 3×9), or
/// `None` if out of range. Same panel placement + 18px pitch as the inventory grid.
pub fn chest_slot_rect(i: usize, screen: (u32, u32), scale: f32) -> Option<SlotRect> {
    if i >= crate::chest::CHEST_SLOTS {
        return None;
    }
    let (ox, oy) = panel_origin(screen, scale);
    let (gx, gy) = CHEST_GRID;
    let col = (i % 9) as f32;
    let row = (i / 9) as f32;
    Some(SlotRect {
        x: ox + (gx + col * PANEL_PITCH) * scale,
        y: oy + (gy + row * PANEL_PITCH) * scale,
        w: SLOT_PX * scale,
        h: SLOT_PX * scale,
    })
}

/// The chest storage-slot index (`0..27`) under the cursor, or `None`. Shared with
/// the App so a visible chest slot is always the one that gets clicked.
pub fn chest_slot_at_cursor(screen: (u32, u32), cursor_px: (f32, f32)) -> Option<usize> {
    let scale = gui_scale(screen);
    let (px, py) = cursor_px;
    (0..crate::chest::CHEST_SLOTS)
        .find(|&i| chest_slot_rect(i, screen, scale).is_some_and(|r| r.contains(px, py)))
}

/// Draw the chest storage slots' icons + counts (chest screen only). Called by
/// [`super::build_ui`] when the open panel is a chest.
pub(super) fn build(ui: &UiSnapshot, build: &mut UiBuild, screen: (u32, u32), scale: f32) {
    let Some(chest) = ui.chest else { return };
    for (i, stack) in chest.slots.iter().enumerate() {
        let Some(stack) = stack else { continue };
        if stack.item == ItemType::Air || stack.count == 0 {
            continue;
        }
        let Some(r) = chest_slot_rect(i, screen, scale) else {
            continue;
        };
        icon::push_slot_icon(build, screen, stack.item, r);
        if stack.count > 1 {
            icon::push_count(&mut build.overlay_verts, screen, stack.count as u32, r, scale);
        }
    }
}
