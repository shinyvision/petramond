//! The open inventory panel: the classic 176×166 art with the main 3×9 grid and
//! the hotbar row. Owns the unified per-slot [`slot_rect`] (the single source of
//! truth shared by the renderer and the App's [`slot_at_cursor`] hit-test — closed
//! HUD slots come from [`super::hotbar`], open slots from here), [`cursor_in_panel`]
//! and the shared per-slot icon draw used by every screen.

use super::super::renderer::UiSnapshot;
use super::{
    gui_scale, hotbar, icon, panel_origin, SlotRect, UiBuild, HOTBAR_LEN, PANEL_H, PANEL_PITCH,
    PANEL_W, SLOT_PX, TOTAL_SLOTS,
};
use crate::item::ItemType;

/// Classic inventory panel interior layout (px), relative to the 176×166 panel's
/// top-left. The main 3×9 grid starts at (8, 84) and the hotbar row at (8, 142),
/// matching the vanilla `inventory.png` art.
const PANEL_GRID_X: f32 = 8.0;
const PANEL_GRID_Y: f32 = 84.0;
const PANEL_HOTBAR_Y: f32 = 142.0;

/// The interior pixel rect of open-panel slot `i` (`0..TOTAL_SLOTS`): the hotbar
/// row at the bottom of the panel, then the main 3×9 grid. Both use the 18px panel
/// pitch (NOT the closed hotbar widget's 20px).
fn panel_slot_rect(i: usize, screen: (u32, u32), scale: f32) -> Option<SlotRect> {
    if i >= TOTAL_SLOTS {
        return None;
    }
    let s = scale;
    let interior = SLOT_PX * s;
    let (ox, oy) = panel_origin(screen, s);
    if i < HOTBAR_LEN {
        // Hotbar row at the bottom of the panel (18px panel pitch, not 20px).
        let col = i as f32;
        let x = ox + (PANEL_GRID_X + col * PANEL_PITCH) * s;
        let y = oy + PANEL_HOTBAR_Y * s;
        Some(SlotRect {
            x,
            y,
            w: interior,
            h: interior,
        })
    } else {
        // Main 3×9 grid, slots 9..36 (18px panel pitch, not 20px).
        let g = i - HOTBAR_LEN;
        let col = (g % 9) as f32;
        let row = (g / 9) as f32;
        let x = ox + (PANEL_GRID_X + col * PANEL_PITCH) * s;
        let y = oy + (PANEL_GRID_Y + row * PANEL_PITCH) * s;
        Some(SlotRect {
            x,
            y,
            w: interior,
            h: interior,
        })
    }
}

/// The interior pixel rect of slot `i` (`0..TOTAL_SLOTS`) for the current screen /
/// open state. When closed, only the 9 hotbar slots have a rect (from
/// [`super::hotbar`]); main-grid slots return `None`. When open, all 36 come from
/// the panel layout. The single source of truth for both rendering and hit-testing.
pub(super) fn slot_rect(i: usize, screen: (u32, u32), open: bool, scale: f32) -> Option<SlotRect> {
    if open {
        panel_slot_rect(i, screen, scale)
    } else {
        // Closed: only the hotbar row is interactive / drawn.
        hotbar::hotbar_slot_rect(i, screen, scale)
    }
}

/// The inventory slot index (`0..TOTAL_SLOTS`) under the cursor, or `None`.
///
/// Pure layout function shared with the App (contract §9): when the inventory is
/// closed only the 9 hotbar slots are hit-testable (main-grid clicks return
/// `None`); when open all 36 slots are. Uses the SAME [`slot_rect`] math the
/// renderer draws with, so a visible slot is always the slot that gets clicked.
pub fn slot_at_cursor(screen: (u32, u32), open: bool, cursor_px: (f32, f32)) -> Option<usize> {
    let scale = gui_scale(screen);
    let (px, py) = cursor_px;
    let limit = if open { TOTAL_SLOTS } else { HOTBAR_LEN };
    (0..limit).find(|&i| slot_rect(i, screen, open, scale).is_some_and(|r| r.contains(px, py)))
}

/// Whether `cursor_px` lies within the open inventory panel rectangle (the
/// classic 176×166 art, centred). Used by the App to tell a click on the panel
/// from a "confidently outside the inventory" click that throws the held stack.
/// Uses the same [`panel_origin`] placement and auto [`gui_scale`] the panel is
/// drawn with. Always `false` for a degenerate zero-size screen.
pub fn cursor_in_panel(screen: (u32, u32), cursor_px: (f32, f32)) -> bool {
    if screen.0 == 0 || screen.1 == 0 {
        return false;
    }
    let scale = gui_scale(screen);
    let (ox, oy) = panel_origin(screen, scale);
    let (px, py) = cursor_px;
    px >= ox && px < ox + PANEL_W * scale && py >= oy && py < oy + PANEL_H * scale
}

/// Draw the inventory slots' item icons + stack counts, shared by every screen:
/// closed = the 9 hotbar slots, open = all 36. Called by [`super::build_ui`] after
/// the screen background, before any screen-specific extra slots.
pub(super) fn build_slots(ui: &UiSnapshot, build: &mut UiBuild, screen: (u32, u32), scale: f32) {
    let slot_count = if ui.open { TOTAL_SLOTS } else { HOTBAR_LEN };
    for i in 0..slot_count {
        let Some((item, count)) = ui.slots[i] else {
            continue;
        };
        if item == ItemType::Air || count == 0 {
            continue;
        }
        let Some(r) = slot_rect(i, screen, ui.open, scale) else {
            continue;
        };
        icon::push_slot_icon(build, screen, item, r);
        if count > 1 {
            icon::push_count(&mut build.overlay_verts, screen, count as u32, r, scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_has_all_36_slot_rects() {
        let scale = gui_scale((1280, 720));
        for i in 0..TOTAL_SLOTS {
            assert!(slot_rect(i, (1280, 720), true, scale).is_some(), "slot {i}");
        }
    }

    /// BUG 2: the OPEN inventory uses the classic 176×166 panel's 18px slot pitch
    /// (NOT the closed hotbar widget's 20px), so all 9 columns stay inside the 176px
    /// panel. With the wrong 20px pitch the 9th slot fell off the panel's right edge.
    #[test]
    fn open_slots_use_panel_pitch_and_fit_within_panel() {
        let screen = (1280u32, 720u32);
        let scale = gui_scale(screen);
        let (ox, _oy) = panel_origin(screen, scale);
        let panel_right = ox + PANEL_W * scale;
        // Open hotbar row AND the main grid: the 9th column's right edge fits inside.
        for &base in &[0usize, HOTBAR_LEN] {
            let last = base + 8; // 9th slot in the row.
            let r = slot_rect(last, screen, true, scale).unwrap();
            assert!(
                r.x + r.w <= panel_right + 1e-3,
                "open 9th slot (base {base}) right {} exceeds panel right {panel_right}",
                r.x + r.w
            );
            // Adjacent open slots step by exactly the 18px panel pitch (× scale).
            let r0 = slot_rect(base, screen, true, scale).unwrap();
            let r1 = slot_rect(base + 1, screen, true, scale).unwrap();
            assert!(
                (r1.x - r0.x - PANEL_PITCH * scale).abs() < 1e-3,
                "open pitch must be {PANEL_PITCH}px"
            );
        }
        // Classic coords: first main-grid slot at panel (8,84), hotbar row at (8,142).
        let (gx, gy) = (
            ox + PANEL_GRID_X * scale,
            panel_origin(screen, scale).1 + PANEL_GRID_Y * scale,
        );
        let grid0 = slot_rect(HOTBAR_LEN, screen, true, scale).unwrap();
        assert!(
            (grid0.x - gx).abs() < 1e-3 && (grid0.y - gy).abs() < 1e-3,
            "main grid origin (8,84)"
        );
        let hb0 = slot_rect(0, screen, true, scale).unwrap();
        let hy = panel_origin(screen, scale).1 + PANEL_HOTBAR_Y * scale;
        assert!(
            (hb0.x - gx).abs() < 1e-3 && (hb0.y - hy).abs() < 1e-3,
            "hotbar row origin (8,142)"
        );
        // Slot interior is the classic 16px (× scale).
        assert!(
            (grid0.w - SLOT_PX * scale).abs() < 1e-3,
            "slot interior 16px"
        );
    }

    #[test]
    fn slot_at_cursor_round_trips_slot_centers() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        // Closed: hotbar slots hit, main grid never.
        for i in 0..HOTBAR_LEN {
            let r = slot_rect(i, screen, false, scale).unwrap();
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert_eq!(slot_at_cursor(screen, false, c), Some(i));
        }
        // Open: all 36 hit.
        for i in 0..TOTAL_SLOTS {
            let r = slot_rect(i, screen, true, scale).unwrap();
            let c = (r.x + r.w * 0.5, r.y + r.h * 0.5);
            assert_eq!(slot_at_cursor(screen, true, c), Some(i));
        }
    }

    #[test]
    fn slot_at_cursor_misses_outside_any_slot() {
        // Top-left corner is outside every slot in both states.
        assert_eq!(slot_at_cursor((1280, 720), false, (0.0, 0.0)), None);
        assert_eq!(slot_at_cursor((1280, 720), true, (0.0, 0.0)), None);
    }

    #[test]
    fn cursor_in_panel_matches_panel_rect() {
        let screen = (1280u32, 720u32);
        let scale = gui_scale(screen);
        let (ox, oy) = panel_origin(screen, scale);
        // Panel centre is inside.
        assert!(cursor_in_panel(
            screen,
            (ox + PANEL_W * scale * 0.5, oy + PANEL_H * scale * 0.5)
        ));
        // Screen corner is confidently outside.
        assert!(!cursor_in_panel(screen, (0.0, 0.0)));
        // Just past the right edge is outside.
        assert!(!cursor_in_panel(
            screen,
            (ox + PANEL_W * scale + 1.0, oy + 1.0)
        ));
        // Every open slot lies within the panel it's drawn in.
        for i in 0..TOTAL_SLOTS {
            let r = slot_rect(i, screen, true, scale).unwrap();
            assert!(
                cursor_in_panel(screen, (r.x + r.w * 0.5, r.y + r.h * 0.5)),
                "slot {i} centre should be inside the panel"
            );
        }
        // Degenerate screen: never inside.
        assert!(!cursor_in_panel((0, 0), (0.0, 0.0)));
    }
}
