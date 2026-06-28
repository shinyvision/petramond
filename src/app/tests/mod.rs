use super::App;
use crate::camera::Camera;
use crate::gui::MenuSlot;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;

mod controls;
mod drops;
mod gui_routing;

impl App {
    /// Route a screen click and then apply the latched container edit / drop, standing in
    /// for the game tick that resolves it in play. Tests assert the resulting inventory /
    /// world state right after, and a real tick interleaves between two clicks (applying
    /// the first before the second is decided), so the per-click apply mirrors play.
    fn click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        let consumed = self.route_screen_click(screen, now);
        self.game.apply_latched_actions_for_test();
        consumed
    }

    /// Right-click counterpart of [`click_screen_for_test`](Self::click_screen_for_test).
    fn right_click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        let consumed = self.route_screen_right_click(screen, now);
        self.game.apply_latched_actions_for_test();
        consumed
    }
}

fn app() -> App {
    App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
}

/// An app whose player holds one full stack in hotbar slot 0 - the starting
/// inventory is empty now, so inventory-interaction tests seed a stack first.
fn app_with_grass() -> App {
    let mut app = app();
    app.game
        .add_to_inventory(ItemStack::new(ItemType::Grass, 64));
    app
}

/// Brute-force a cursor pixel that the open GUI's hit-test resolves to `want`,
/// using the REAL baked geometry so tests never pin manifest pixel positions.
fn cursor_over_menu(screen: (u32, u32), kind: crate::gui::GuiKind, want: MenuSlot) -> (f32, f32) {
    for y in 0..screen.1 {
        for x in 0..screen.0 {
            let c = (x as f32 + 0.5, y as f32 + 0.5);
            if crate::gui::hit(kind, screen, c) == Some(want) {
                return c;
            }
        }
    }
    panic!("no cursor position maps to {want:?} in {kind:?}");
}

fn cursor_over_slot(screen: (u32, u32), slot: usize) -> (f32, f32) {
    cursor_over_menu(
        screen,
        crate::gui::GuiKind::Inventory,
        MenuSlot::Inventory(slot),
    )
}

fn cursor_over_craft(
    screen: (u32, u32),
    kind: crate::gui::GuiKind,
    hit: crate::gui::CraftHit,
) -> (f32, f32) {
    cursor_over_menu(screen, kind, MenuSlot::Craft(hit))
}

/// A point inside the panel rectangle that is NOT over any slot.
fn panel_gap_point(screen: (u32, u32)) -> (f32, f32) {
    let kind = crate::gui::GuiKind::Inventory;
    for y in 0..screen.1 {
        for x in 0..screen.0 {
            let c = (x as f32 + 0.5, y as f32 + 0.5);
            if crate::gui::panel_contains(kind, screen, c)
                && crate::gui::hit(kind, screen, c).is_none()
            {
                return c;
            }
        }
    }
    panic!("no in-panel, off-slot point found");
}
