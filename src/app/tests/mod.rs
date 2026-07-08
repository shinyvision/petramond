use super::App;
use crate::camera::Camera;
use crate::controls::PointerButton;
use crate::game::Game;
use crate::gui::MenuSlot;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::server::game::ServerGame;
use crate::server::handle::LoopbackServer;

mod connect;
mod controls;
mod drops;
mod gui_routing;
mod overlays;
mod sounds;

impl App {
    fn game(&self) -> &Game {
        self.game.as_ref().expect("test app has a loaded game")
    }

    /// Solve one input-free document frame for the open menu so its slot rects
    /// are available on `self.ui.out()`.
    fn solve_menu_frame_for_test(&mut self, screen: (u32, u32)) {
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.drive_doc_menu(kind, screen, 0.0);
    }
}

/// The app test fixture: a real [`App`] whose game session rides a LOOPBACK
/// server pipe, with the `ServerGame` held here — the same shape as the game
/// tests' `TestGame` (`src/game/tests/common.rs`), so app tests keep driving
/// latched actions and asserting session state synchronously.
struct TestApp {
    app: App,
    server: ServerGame,
    #[allow(dead_code)]
    pipe: LoopbackServer,
}

impl std::ops::Deref for TestApp {
    type Target = App;
    fn deref(&self) -> &App {
        &self.app
    }
}

impl std::ops::DerefMut for TestApp {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.app
    }
}

impl TestApp {
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

    fn press_screen_for_test(&mut self, screen: (u32, u32), now: f64, button: PointerButton) -> bool {
        if !self.screen.ui_open() {
            return false;
        }
        let kind = self.doc_ui_kind().expect("open menu is document-backed");
        self.app.set_pointer_button(button, true);
        self.app.set_pointer_button(button, false);
        self.app.drive_doc_menu(kind, screen, now);
        self.apply_latched_actions_for_test();
        true
    }

    /// Flush the game's queued messages to the server, apply the latched
    /// actions, and refresh the replicated read models — what the game tests'
    /// harness does, reached through the App.
    fn apply_latched_actions_for_test(&mut self) {
        let game = self.app.game.as_mut().expect("test app has a loaded game");
        for msg in game.take_outbox_for_test() {
            self.server.apply_message(0, msg);
        }
        self.server.apply_latched_actions_for_test();
        // Refresh the replicated self/menu views the way the next batch would.
        self.server.sessions[0].last_sent_inventory_revision = None;
        let state = self.server.build_self_state(0);
        let sync = self.server.build_menu_sync(0);
        game.apply_views_for_test(&state, sync);
    }

    /// The SESSION inventory — the authoritative one the sim mutates.
    fn inventory(&self) -> &Inventory {
        &self.server.sessions[0].player.inventory
    }

    fn add_to_inventory(&mut self, stack: ItemStack) {
        self.server.sessions[0].player.inventory.add(stack);
    }
}

fn app() -> TestApp {
    let (server, bootstrap) = crate::game::session::build_session("", 1, 1);
    let (handle, pipe) = crate::server::handle::ServerHandle::loopback();
    let game = Game::assemble(
        Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0),
        handle,
        bootstrap,
    );
    let mut app = App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1);
    app.adopt_game(game);
    TestApp { app, server, pipe }
}

/// An app whose player holds one full stack in hotbar slot 0 - the starting
/// inventory is empty now, so inventory-interaction tests seed a stack first.
fn app_with_grass() -> TestApp {
    let mut app = app();
    app.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Grass, 64));
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
            return (r.x as f32 + r.w as f32 * 0.5, r.y as f32 + r.h as f32 * 0.5);
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
