//! Application shell for the native desktop host.
//!
//! The app owns window-level state: current screen, input aggregation, cursor
//! policy, frame time, and renderer handoff. The voxel demo itself lives in
//! `game`, and first-person hand animation lives in the renderer presentation
//! layer.

mod gui_router;
mod input;
mod menu_lifecycle;
mod pointer;
mod presentation_events;
mod render;
mod screen;
mod ui_snapshot;
mod update;

pub use screen::{AppScreen, CursorPolicy};

use crate::app::gui_router::GuiRouter;
use crate::app::input::{ControlEvent, InputController};
use crate::app::pointer::PointerState;
use crate::audio::Audio;
use crate::camera::Camera;
use crate::controls::{Control, Modifiers};
use crate::game::presentation::GamePresentationScratch;
use crate::game::{CameraPose, Game};
use crate::render::Scene;

pub struct App {
    game: Game,
    /// Reusable builder for neutral per-frame presentation data read from the game.
    presentation: GamePresentationScratch,
    /// Render-side translation of neutral per-frame presentation data into the
    /// renderer's wire structs.
    scene: Scene,
    /// Client-side sound engine. Drains the sim's per-tick [`crate::audio::SoundEvent`]s
    /// each frame and plays them; never part of the deterministic simulation.
    audio: Audio,
    last: f64,
    input: InputController,
    pointer: PointerState,
    gui_router: GuiRouter,
    screen: AppScreen,
    /// Physical Ctrl/Shift modifier state from the windowing system, tracked apart
    /// from the rebindable Sprint/Sneak controls. Drives UI modifiers (Ctrl =
    /// drop whole stack, Shift = inventory quick-move).
    modifiers: Modifiers,
    /// Set whenever input or a state change means the next frame would differ from
    /// the last drawn one. Drives redraw-on-demand: the host draws only when this (or
    /// camera motion / client-frame activity) holds, instead of every frame.
    /// Consumed (peeked-and-cleared) by [`update`](Self::update).
    dirty: bool,
    /// `now_seconds` of the last [`render`](Self::render). Render runs on demand, not
    /// once per update, so the held-item animation advances by its own delta.
    last_render: f64,
    /// Camera pose at the last render, to detect a moved view (the dominant redraw
    /// trigger) without redrawing an unchanged one. Standing still reproduces
    /// bit-identical values, so equality means "view unchanged".
    last_pose: Option<CameraPose>,
    /// First-person hand-animation triggers latched since the last render, so a
    /// swing/place/break begun on an un-drawn update isn't lost before the next draw.
    hand: HandTriggers,
}

/// One-shot first-person hand-animation triggers, latched by [`App::update`] and
/// consumed by the next [`App::render`]. OR-merged across updates so none is dropped
/// when several sim updates run between two draws.
#[derive(Default, Copy, Clone)]
struct HandTriggers {
    broke: bool,
    placed: bool,
    swung: bool,
}

impl App {
    pub fn new(cam: Camera, world_name: &str, seed: u32, render_dist: i32) -> Self {
        Self {
            game: Game::new(cam, world_name, seed, render_dist),
            presentation: GamePresentationScratch::new(),
            scene: Scene::new(),
            audio: Audio::new(),
            last: now_seconds(),
            input: InputController::default(),
            pointer: PointerState::default(),
            gui_router: GuiRouter::default(),
            screen: AppScreen::Game,
            modifiers: Modifiers::default(),
            // Draw the first frame unconditionally (also forced by `last_pose: None`).
            dirty: true,
            last_render: now_seconds(),
            last_pose: None,
            hand: HandTriggers::default(),
        }
    }

    /// Flush the world to disk on quit. The `WorldSave` I/O thread is joined when
    /// the `App` (and the `World` it owns) drops, after this queues the writes.
    pub fn save_on_exit(&mut self) {
        self.game.save_all();
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
        self.dirty = true;
    }

    /// Apply a shared control event. Returns false only when the app did not
    /// consume the control, e.g. Escape with no screen open on native.
    pub fn handle_control(&mut self, control: Control, down: bool) -> bool {
        // Any control edge (move, look-bind, hotbar, menu toggle, …) changes what the
        // next frame shows, so force a redraw; movement is also caught by camera motion.
        self.dirty = true;
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
            ControlEvent::DropItem => {
                // Q drops the held item only while playing (not in a menu). The
                // physical Ctrl modifier (not the sprint key) selects whole-stack.
                if self.screen.gameplay_enabled() {
                    self.game.drop_selected_item(self.modifiers.ctrl);
                }
                true
            }
        }
    }

    /// Update the tracked physical keyboard modifiers (Ctrl / Shift) from the
    /// platform's modifier-changed event. Independent of the rebindable
    /// Sprint/Sneak controls.
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
        self.dirty = true;
    }

    /// Does pending input want a frame? Peeked (not cleared) by the host between updates
    /// to serve input promptly without busy-waiting; [`update`](Self::update) clears it.
    #[inline]
    pub fn wants_redraw(&self) -> bool {
        self.dirty
    }
}

fn now_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Control;
    use crate::gui::MenuSlot;
    use crate::item::{ItemStack, ItemType};
    use crate::mathh::Vec3;
    use crate::player::PlayerMode;

    fn app() -> App {
        App::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
    }

    /// An app whose player holds one full stack in hotbar slot 0 — the starting
    /// inventory is empty now, so inventory-interaction tests seed a stack first.
    fn app_with_grass() -> App {
        let mut app = app();
        app.game
            .add_to_inventory(ItemStack::new(ItemType::Grass, 64));
        app
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
        assert!(!app.pointer.is_grabbing());
    }

    #[test]
    fn escape_closes_open_inventory_and_regrabs() {
        let mut app = app();
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        assert!(!app.pointer.is_grabbing());

        assert!(app.handle_control(Control::CloseScreen, true));
        assert!(!app.inventory_open());
        assert!(app.pointer.is_grabbing());
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

    /// Brute-force a cursor pixel that the open GUI's hit-test resolves to `want`,
    /// using the REAL baked geometry so tests never pin manifest pixel positions.
    fn cursor_over_menu(
        screen: (u32, u32),
        kind: crate::gui::GuiKind,
        want: MenuSlot,
    ) -> (f32, f32) {
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

    #[test]
    fn craft_slot_clicks_route_through_to_crafting() {
        use crate::gui::{CraftHit, GuiKind};
        let mut app = app();
        // Give the player one oak log and open the inventory (2×2 crafting).
        app.game
            .add_to_inventory(ItemStack::new(ItemType::OakLog, 1));
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280u32, 720u32);

        // Pick the log up from inventory slot 0.
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.click_screen_for_test(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());

        // Drop it into the first 2×2 craft input cell -> planks preview appears.
        let cc = cursor_over_craft(screen, GuiKind::Inventory, CraftHit::Input(0));
        app.set_cursor_position(cc.0, cc.1);
        app.click_screen_for_test(screen, 0.1);
        assert!(
            app.game.inventory().cursor().is_none(),
            "log placed into the craft cell"
        );
        assert_eq!(
            app.game.menu_read_model().craft.result().map(|s| s.item),
            Some(ItemType::OakPlanks)
        );

        // Click the result slot: 4 planks land on the cursor, ingredients consumed.
        let rc = cursor_over_craft(screen, GuiKind::Inventory, CraftHit::Result);
        app.set_cursor_position(rc.0, rc.1);
        app.click_screen_for_test(screen, 0.2);
        assert_eq!(
            app.game.inventory().cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        assert!(app.game.menu_read_model().craft.result().is_none());
    }

    #[test]
    fn closing_a_menu_returns_craft_grid_items_to_inventory() {
        let mut app = app();
        app.game
            .add_to_inventory(ItemStack::new(ItemType::OakLog, 2));
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280u32, 720u32);
        // Move the logs onto the cursor and into a craft cell.
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.click_screen_for_test(screen, 0.0);
        let cc = cursor_over_craft(
            screen,
            crate::gui::GuiKind::Inventory,
            crate::gui::CraftHit::Input(0),
        );
        app.set_cursor_position(cc.0, cc.1);
        app.click_screen_for_test(screen, 0.1);
        // Close with Escape: the logs return to the inventory.
        assert!(app.handle_control(Control::CloseScreen, true));
        assert!(!app.inventory_open());
        let logs: u32 = (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| app.game.inventory().slot(i))
            .filter(|s| s.item == ItemType::OakLog)
            .map(|s| s.count as u32)
            .sum();
        assert_eq!(logs, 2, "craft-grid logs came back to the inventory");
    }

    #[test]
    fn closing_a_menu_stashes_the_cursor_stack() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280u32, 720u32);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.click_screen_for_test(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());

        assert!(app.handle_control(Control::CloseScreen, true));

        assert!(app.game.inventory().cursor().is_none());
        let grass: u32 = (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| app.game.inventory().slot(i))
            .filter(|s| s.item == ItemType::Grass)
            .map(|s| s.count as u32)
            .sum();
        assert_eq!(grass, 64, "cursor stack was parked back in inventory");
    }

    #[test]
    fn route_inventory_click_open_picks_up_slot_stack() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);

        assert!(app.game.inventory().cursor().is_none());
        let item0 = app.game.inventory().slot(0).unwrap().item;

        let consumed = app.click_screen_for_test(screen, 0.0);
        assert!(consumed);
        assert!(app.game.inventory().slot(0).is_none());
        assert_eq!(app.game.inventory().cursor().unwrap().item, item0);
    }

    #[test]
    fn fast_double_click_keeps_stack_on_cursor_to_gather() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);

        // First click picks the stack up; a second click within the double-click
        // window gathers matching items instead of dropping it back — so the stack
        // stays on the cursor and the source slot stays empty.
        app.click_screen_for_test(screen, 0.0);
        app.click_screen_for_test(screen, 0.1);
        assert!(
            app.game.inventory().cursor().is_some(),
            "stack stays on the cursor"
        );
        assert!(
            app.game.inventory().slot(0).is_none(),
            "source slot stays empty"
        );
    }

    #[test]
    fn slow_second_click_drops_the_stack_back() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);

        // Two clicks spaced beyond the double-click window: the second is a normal
        // click that drops the held stack back into the now-empty slot.
        app.click_screen_for_test(screen, 0.0);
        app.click_screen_for_test(screen, 1.0);
        assert!(
            app.game.inventory().cursor().is_none(),
            "stack dropped back"
        );
        assert!(app.game.inventory().slot(0).is_some(), "slot refilled");
    }

    #[test]
    fn fast_click_on_a_different_slot_is_not_a_double_click() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280, 720);
        // Pick up slot 0's stack.
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.click_screen_for_test(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());

        // A fast click on a DIFFERENT slot is a normal drop, not a gather: the held
        // stack lands in the first (empty) main-grid slot.
        let dest = crate::inventory::HOTBAR_LEN;
        let (dx, dy) = cursor_over_slot(screen, dest);
        app.set_cursor_position(dx, dy);
        app.click_screen_for_test(screen, 0.05);
        assert!(
            app.game.inventory().cursor().is_none(),
            "stack dropped into the new slot"
        );
        assert!(app.game.inventory().slot(dest).is_some());
    }

    #[test]
    fn route_inventory_click_closed_is_a_noop() {
        let mut app = app();
        assert!(!app.inventory_open());
        let before = app.game.inventory().slot(0).map(|s| s.count);
        let consumed = app.click_screen_for_test((1280, 720), 0.0);
        assert!(!consumed);
        assert!(app.game.inventory().cursor().is_none());
        assert_eq!(app.game.inventory().slot(0).map(|s| s.count), before);
    }

    #[test]
    fn q_drops_one_held_item_while_playing() {
        let mut app = app_with_grass();
        let before = app.game.inventory().selected().unwrap().count;
        app.handle_control(Control::DropItem, true);
        assert_eq!(
            app.game.inventory().selected().unwrap().count,
            before,
            "drop is latched until the fixed tick applies it"
        );
        app.game.apply_latched_actions_for_test();
        assert_eq!(app.game.inventory().selected().unwrap().count, before - 1);
    }

    #[test]
    fn ctrl_q_drops_whole_held_stack_while_playing() {
        let mut app = app_with_grass();
        assert!(app.game.inventory().selected().is_some());
        // Physical Ctrl modifier held (NOT via the sprint control).
        app.set_modifiers(Modifiers {
            ctrl: true,
            shift: false,
        });
        app.handle_control(Control::DropItem, true);
        assert!(
            app.game.inventory().selected().is_some(),
            "drop-all is latched until the fixed tick applies it"
        );
        app.game.apply_latched_actions_for_test();
        assert!(
            app.game.inventory().selected().is_none(),
            "whole stack dropped"
        );
    }

    #[test]
    fn q_drops_one_even_while_sprinting_when_ctrl_not_tracked() {
        // Holding the sprint *control* must NOT turn Q into a drop-all: only the
        // physical Ctrl modifier does. Guards the decoupling from the keybind.
        let mut app = app_with_grass();
        app.handle_control(Control::Sprint, true); // sprint action held
        let before = app.game.inventory().selected().unwrap().count;
        app.handle_control(Control::DropItem, true);
        assert_eq!(
            app.game.inventory().selected().unwrap().count,
            before,
            "drop is latched until the fixed tick applies it"
        );
        app.game.apply_latched_actions_for_test();
        assert_eq!(
            app.game.inventory().selected().unwrap().count,
            before - 1,
            "sprint key alone drops one, not the whole stack"
        );
    }

    #[test]
    fn q_does_not_drop_while_inventory_open() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let before = app.game.inventory().selected().map(|s| s.count);
        app.handle_control(Control::DropItem, true);
        assert_eq!(app.game.inventory().selected().map(|s| s.count), before);
    }

    #[test]
    fn route_inventory_right_click_splits_slot_stack() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        // Slot 0 starts at 64; right-click drags off the larger half (32).
        let consumed = app.right_click_screen_for_test(screen, 0.0);
        assert!(consumed);
        assert_eq!(app.game.inventory().cursor().unwrap().count, 32);
        assert_eq!(app.game.inventory().slot(0).unwrap().count, 32);
    }

    #[test]
    fn route_inventory_right_click_closed_falls_through_to_placement() {
        // Closed inventory: a right-click is NOT consumed, so it can place a block.
        let mut app = app();
        assert!(!app.inventory_open());
        assert!(!app.right_click_screen_for_test((1280, 720), 0.0));
    }

    #[test]
    fn route_inventory_shift_click_moves_hotbar_to_main_grid() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        // Physical Shift modifier held (NOT via the sneak control).
        app.set_modifiers(Modifiers {
            ctrl: false,
            shift: true,
        });
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        let item0 = app.game.inventory().slot(0).unwrap().item;
        app.click_screen_for_test(screen, 0.0);
        assert!(
            app.game.inventory().slot(0).is_none(),
            "hotbar slot emptied"
        );
        assert_eq!(
            app.game
                .inventory()
                .slot(crate::inventory::HOTBAR_LEN)
                .unwrap()
                .item,
            item0,
            "moved to the first main-grid slot"
        );
    }

    #[test]
    fn route_click_outside_panel_throws_held_stack() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280, 720);
        // Drag slot 0's stack onto the cursor.
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.click_screen_for_test(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());
        // Click the top-left corner: confidently outside the inventory panel.
        app.set_cursor_position(0.0, 0.0);
        app.click_screen_for_test(screen, 0.1);
        assert!(
            app.game.inventory().cursor().is_none(),
            "held stack thrown out of the inventory"
        );
    }

    #[test]
    fn route_click_on_panel_background_does_not_throw() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.click_screen_for_test(screen, 0.0); // pick up the stack
        assert!(app.game.inventory().cursor().is_some());
        // A point inside the panel but on no slot: the held stack is kept.
        let inside_panel_gap = panel_gap_point(screen);
        app.set_cursor_position(inside_panel_gap.0, inside_panel_gap.1);
        app.click_screen_for_test(screen, 0.1);
        assert!(
            app.game.inventory().cursor().is_some(),
            "click on panel art must not throw the stack"
        );
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
}
