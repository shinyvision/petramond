use super::App;
use crate::camera::Camera;
use crate::controls::PointerButton;
use crate::game::Game;
use crate::gui::MenuSlot;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;

mod controls;
mod drops;
mod gui_routing;
mod overlays;

impl App {
    fn game(&self) -> &Game {
        self.game.as_ref().expect("test app has a loaded game")
    }

    fn game_mut(&mut self) -> &mut Game {
        self.game.as_mut().expect("test app has a loaded game")
    }

    /// Click the open document menu at the current cursor and then apply the
    /// latched container edit / drop, standing in for the game tick that
    /// resolves it in play. Returns whether a document menu consumed the click
    /// (false with no menu open, so the click would fall through to gameplay).
    fn click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.press_screen_for_test(screen, now, PointerButton::Primary)
    }

    /// Right-click counterpart of [`click_screen_for_test`](Self::click_screen_for_test).
    fn right_click_screen_for_test(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.press_screen_for_test(screen, now, PointerButton::Secondary)
    }

    fn press_screen_for_test(
        &mut self,
        screen: (u32, u32),
        now: f64,
        button: PointerButton,
    ) -> bool {
        if !self.screen.ui_open() {
            return false;
        }
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.set_pointer_button(button, true);
        self.set_pointer_button(button, false);
        self.drive_doc_menu(kind, screen, now);
        self.game_mut().apply_latched_actions_for_test();
        true
    }

    /// Solve one input-free document frame for the open menu so its slot rects
    /// are available on `self.ui.out()`.
    fn solve_menu_frame_for_test(&mut self, screen: (u32, u32)) {
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.drive_doc_menu(kind, screen, 0.0);
    }
}

fn app() -> App {
    App::new_in_game(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
}

/// An app whose player holds one full stack in hotbar slot 0 - the starting
/// inventory is empty now, so inventory-interaction tests seed a stack first.
fn app_with_grass() -> App {
    let mut app = app();
    app.game_mut()
        .add_to_inventory(ItemStack::new(ItemType::Grass, 64));
    app
}

/// The cursor pixel over the slot cell the open menu's DOCUMENT resolves to
/// `want`, from the real solved layout — tests never pin document pixel
/// positions.
fn cursor_over_menu(app: &mut App, screen: (u32, u32), want: MenuSlot) -> (f32, f32) {
    app.solve_menu_frame_for_test(screen);
    for slot in &app.ui.out().slots {
        let hit = crate::gui::Role::from_key(&slot.role)
            .and_then(|role| role.menu_slot(slot.index as usize));
        if hit == Some(want) {
            let r = slot.rect;
            return (
                r.x as f32 + r.w as f32 * 0.5,
                r.y as f32 + r.h as f32 * 0.5,
            );
        }
    }
    panic!("no document slot cell maps to {want:?}");
}

fn cursor_over_slot(app: &mut App, screen: (u32, u32), slot: usize) -> (f32, f32) {
    cursor_over_menu(app, screen, MenuSlot::Inventory(slot))
}

fn cursor_over_craft(app: &mut App, screen: (u32, u32), hit: crate::gui::CraftHit) -> (f32, f32) {
    cursor_over_menu(app, screen, MenuSlot::Craft(hit))
}

/// A point inside the open menu's panel rectangle that is NOT over any slot.
fn panel_gap_point(app: &mut App, screen: (u32, u32)) -> (f32, f32) {
    app.solve_menu_frame_for_test(screen);
    let out = app.ui.out();
    let panel = out.panel_rect;
    for y in panel.y..panel.y + panel.h {
        for x in panel.x..panel.x + panel.w {
            let c = (x as f32 + 0.5, y as f32 + 0.5);
            let on_slot = out.slots.iter().any(|s| {
                c.0 >= s.rect.x as f32
                    && c.0 < (s.rect.x + s.rect.w) as f32
                    && c.1 >= s.rect.y as f32
                    && c.1 < (s.rect.y + s.rect.h) as f32
            });
            if !on_slot {
                return c;
            }
        }
    }
    panic!("no in-panel, off-slot point found");
}
