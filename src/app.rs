//! Application shell shared by native and web.
//!
//! The app owns window-level state: current screen, input aggregation, cursor
//! policy, frame time, and renderer handoff. The voxel demo itself lives in
//! `game`, and first-person hand animation lives in the renderer presentation
//! layer.

mod input;
mod screen;

pub use screen::{AppScreen, CursorPolicy};

use crate::app::input::{ControlEvent, InputController};
use crate::camera::Camera;
use crate::controls::{Control, PointerButton};
use crate::game::{Game, GameInput};
use crate::render::{HeldItemFrame, Renderer, UiFrame};

pub struct App {
    game: Game,
    last: f64,
    input: InputController,
    pointer: PointerState,
    screen: AppScreen,
    /// Set when the inventory opens so the UI cursor is centred on the next tick,
    /// where the renderer surface size is known.
    recenter_cursor: bool,
}

#[derive(Default, Copy, Clone, Debug)]
struct PointerState {
    dx: f32,
    dy: f32,
    grabbing: bool,
    left_click: bool,
    right_click: bool,
    left_held: bool,
    scroll_delta: f32,
    cursor_x: f32,
    cursor_y: f32,
}

impl App {
    pub fn new(cam: Camera, seed: u32, render_dist: i32) -> Self {
        Self {
            game: Game::new(cam, seed, render_dist),
            last: now_seconds(),
            input: InputController::default(),
            pointer: PointerState::default(),
            screen: AppScreen::Game,
            recenter_cursor: false,
        }
    }

    #[inline]
    pub fn screen(&self) -> AppScreen {
        self.screen
    }

    #[inline]
    pub fn inventory_open(&self) -> bool {
        self.screen.inventory_open()
    }

    #[inline]
    pub fn cursor_policy(&self) -> CursorPolicy {
        CursorPolicy::for_screen(self.screen)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.game.set_aspect(width as f32 / height.max(1) as f32);
    }

    /// Apply a shared control event. Returns false only when the app did not
    /// consume the control, e.g. Escape with no screen open on native.
    pub fn handle_control(&mut self, control: Control, down: bool) -> bool {
        let Some(event) = self.input.set_control(control, down) else {
            return true;
        };

        match event {
            ControlEvent::ToggleInventory => {
                self.toggle_inventory();
                true
            }
            ControlEvent::TogglePlayerMode => {
                self.game.toggle_player_mode();
                true
            }
            ControlEvent::CloseScreen => self.close_screen(),
            ControlEvent::SelectHotbar(slot) => {
                if self.screen.gameplay_enabled() {
                    self.game.set_active_hotbar(slot);
                }
                true
            }
        }
    }

    pub fn set_pointer_grabbed(&mut self, grabbed: bool) {
        self.pointer.grabbing = grabbed;
    }

    pub fn add_pointer_motion(&mut self, dx: f32, dy: f32) {
        self.pointer.dx += dx;
        self.pointer.dy += dy;
        self.pointer.grabbing = true;
    }

    pub fn set_cursor_position(&mut self, x: f32, y: f32) {
        self.pointer.cursor_x = x;
        self.pointer.cursor_y = y;
    }

    pub fn set_pointer_button(&mut self, button: PointerButton, down: bool) {
        match (button, down) {
            (PointerButton::Primary, true) => {
                self.pointer.left_click = true;
                self.pointer.left_held = true;
                self.pointer.grabbing = true;
            }
            (PointerButton::Primary, false) => {
                self.pointer.left_held = false;
            }
            (PointerButton::Secondary, true) => {
                self.pointer.right_click = true;
                self.pointer.grabbing = true;
            }
            (PointerButton::Secondary, false) => {}
        }
    }

    pub fn add_scroll_delta(&mut self, delta: f32) {
        self.pointer.scroll_delta += delta;
    }

    pub fn tick(&mut self, renderer: &mut Renderer) {
        let now = now_seconds();
        let dt = (now - self.last) as f32;
        self.last = now;

        let screen_size = renderer.screen_size();
        if self.recenter_cursor {
            self.pointer.cursor_x = screen_size.0 as f32 * 0.5;
            self.pointer.cursor_y = screen_size.1 as f32 * 0.5;
            self.recenter_cursor = false;
        }

        if self.pointer.left_click && self.route_screen_click(screen_size) {
            self.pointer.left_click = false;
        }

        let game_input = self.take_game_input();
        let events = self.game.tick(dt, &game_input);
        self.pointer.clear_edges();

        let environment = self.game.environment(now);
        renderer.update_uniforms(
            self.game.camera(),
            environment.fog,
            environment.time,
            environment.underwater,
        );
        renderer.set_selection(self.game.selection());
        renderer.set_break_overlay(self.game.break_overlay_view());
        renderer.set_held_item(HeldItemFrame {
            item: self.game.selected_item(),
            mining: self.game.is_mining(),
            broke_block: events.broke_block,
            placed: events.placed_block,
            dt,
        });
        renderer.set_held_item_light(self.game.held_item_skylight());

        renderer.set_item_entities(self.game.item_entity_instances());
        renderer.set_particles(self.game.particle_instances());
        renderer.set_ui(UiFrame {
            open: self.screen.inventory_open(),
            inv: self.game.inventory(),
            screen: screen_size,
            cursor_px: (self.pointer.cursor_x, self.pointer.cursor_y),
        });

        renderer.sync_meshes(self.game.world_mut());
        renderer.update_section_visibility(self.game.world_mut());
        renderer.render();
    }

    fn take_game_input(&mut self) -> GameInput {
        let gameplay_enabled = self.screen.gameplay_enabled();
        let look_delta = if gameplay_enabled && self.pointer.grabbing {
            (self.pointer.dx, self.pointer.dy)
        } else {
            (0.0, 0.0)
        };
        self.pointer.dx = 0.0;
        self.pointer.dy = 0.0;

        let hotbar_scroll = if gameplay_enabled {
            self.pointer.take_scroll_step()
        } else {
            self.pointer.scroll_delta = 0.0;
            0
        };

        GameInput {
            gameplay_enabled,
            movement: self.input.movement(),
            look_delta,
            hotbar_scroll,
            break_held: self.pointer.left_held,
            place_clicked: self.pointer.right_click,
        }
    }

    fn toggle_inventory(&mut self) {
        if self.screen.inventory_open() {
            self.close_inventory();
        } else {
            self.open_inventory();
        }
    }

    fn open_inventory(&mut self) {
        self.screen = AppScreen::Inventory;
        self.pointer.grabbing = false;
        self.recenter_cursor = true;
    }

    fn close_inventory(&mut self) {
        self.screen = AppScreen::Game;
        self.pointer.grabbing = true;
    }

    fn close_screen(&mut self) -> bool {
        if self.screen.inventory_open() {
            self.close_inventory();
            true
        } else {
            false
        }
    }

    fn route_screen_click(&mut self, screen: (u32, u32)) -> bool {
        if !self.screen.inventory_open() {
            return false;
        }
        if let Some(i) = crate::render::slot_at_cursor(
            screen,
            true,
            (self.pointer.cursor_x, self.pointer.cursor_y),
        ) {
            self.game.click_inventory_slot(i);
        }
        true
    }
}

impl PointerState {
    fn take_scroll_step(&mut self) -> i32 {
        let step = if self.scroll_delta > 0.0 {
            1
        } else if self.scroll_delta < 0.0 {
            -1
        } else {
            0
        };
        self.scroll_delta = 0.0;
        step
    }

    fn clear_edges(&mut self) {
        self.left_click = false;
        self.right_click = false;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn now_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(target_arch = "wasm32")]
fn now_seconds() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now() / 1000.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Control;
    use crate::mathh::Vec3;
    use crate::player::PlayerMode;

    fn app() -> App {
        App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1, 1)
    }

    #[test]
    fn ctrl_y_toggles_player_mode_once_per_chord() {
        let mut app = app();
        assert_eq!(app.game.player_mode(), PlayerMode::Survival);

        app.handle_control(Control::Sprint, true);
        app.handle_control(Control::TogglePlayerMode, true);
        assert_eq!(app.game.player_mode(), PlayerMode::Spectator);

        app.handle_control(Control::TogglePlayerMode, true);
        app.handle_control(Control::Sprint, true);
        assert_eq!(app.game.player_mode(), PlayerMode::Spectator);

        app.handle_control(Control::TogglePlayerMode, false);
        app.handle_control(Control::TogglePlayerMode, true);
        assert_eq!(app.game.player_mode(), PlayerMode::Survival);

        app.handle_control(Control::Sprint, false);
        app.handle_control(Control::TogglePlayerMode, false);
        app.handle_control(Control::TogglePlayerMode, true);
        assert_eq!(app.game.player_mode(), PlayerMode::Survival);
    }

    #[test]
    fn inventory_toggle_is_once_per_press() {
        let mut app = app();
        assert!(!app.inventory_open());

        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());

        app.handle_control(Control::ToggleInventory, false);
        app.handle_control(Control::ToggleInventory, true);
        assert!(!app.inventory_open());
    }

    #[test]
    fn opening_inventory_releases_grab() {
        let mut app = app();
        app.set_pointer_grabbed(true);
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        assert!(!app.pointer.grabbing);
    }

    #[test]
    fn escape_closes_open_inventory_and_regrabs() {
        let mut app = app();
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        assert!(!app.pointer.grabbing);

        assert!(app.handle_control(Control::CloseScreen, true));
        assert!(!app.inventory_open());
        assert!(app.pointer.grabbing);
    }

    #[test]
    fn escape_with_inventory_closed_is_not_consumed() {
        let mut app = app();
        assert!(!app.inventory_open());
        assert!(!app.handle_control(Control::CloseScreen, true));
        assert!(!app.inventory_open());
    }

    #[test]
    fn digit_controls_select_hotbar_slot() {
        let mut app = app();
        app.handle_control(Control::SelectHotbar(4), true);
        assert_eq!(app.game.inventory().active_slot(), 4);
        app.handle_control(Control::SelectHotbar(0), true);
        assert_eq!(app.game.inventory().active_slot(), 0);
        app.handle_control(Control::SelectHotbar(8), true);
        assert_eq!(app.game.inventory().active_slot(), 8);
    }

    #[test]
    fn digit_controls_ignored_while_inventory_open() {
        let mut app = app();
        app.handle_control(Control::SelectHotbar(2), true);
        assert_eq!(app.game.inventory().active_slot(), 2);
        app.handle_control(Control::ToggleInventory, true);
        app.handle_control(Control::SelectHotbar(6), true);
        assert_eq!(app.game.inventory().active_slot(), 2);
    }

    fn cursor_over_slot(screen: (u32, u32), slot: usize) -> (f32, f32) {
        for y in 0..screen.1 {
            for x in 0..screen.0 {
                let c = (x as f32 + 0.5, y as f32 + 0.5);
                if crate::render::slot_at_cursor(screen, true, c) == Some(slot) {
                    return c;
                }
            }
        }
        panic!("no cursor position maps to slot {slot}");
    }

    #[test]
    fn route_inventory_click_open_picks_up_slot_stack() {
        let mut app = app();
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);

        assert!(app.game.inventory().cursor().is_none());
        let item0 = app.game.inventory().slot(0).unwrap().item;

        let consumed = app.route_screen_click(screen);
        assert!(consumed);
        assert!(app.game.inventory().slot(0).is_none());
        assert_eq!(app.game.inventory().cursor().unwrap().item, item0);
    }

    #[test]
    fn route_inventory_click_closed_is_a_noop() {
        let mut app = app();
        assert!(!app.inventory_open());
        let before = app.game.inventory().slot(0).map(|s| s.count);
        let consumed = app.route_screen_click((1280, 720));
        assert!(!consumed);
        assert!(app.game.inventory().cursor().is_none());
        assert_eq!(app.game.inventory().slot(0).map(|s| s.count), before);
    }
}
