//! Application shell for the native desktop host.
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
use crate::controls::{Control, Modifiers, PointerButton};
use crate::game::{ContainerTarget, Game, GameInput, MenuSlot};
use crate::render::{HeldItemFrame, Renderer, Scene, UiFrame};

pub struct App {
    game: Game,
    /// Render-side translation of the sim's per-frame world data (dropped items,
    /// particles, chests, held-item light) into the renderer's wire structs.
    scene: Scene,
    last: f64,
    input: InputController,
    pointer: PointerState,
    screen: AppScreen,
    /// Physical Ctrl/Shift modifier state from the windowing system, tracked apart
    /// from the rebindable Sprint/Sneak controls. Drives UI modifiers (Ctrl =
    /// drop whole stack, Shift = inventory quick-move).
    modifiers: Modifiers,
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
    /// Inventory slot of the last left-click and when it happened, for
    /// double-click detection. `None` means no streak is in progress.
    last_click_slot: Option<usize>,
    last_click_time: f64,
}

impl App {
    pub fn new(cam: Camera, world_name: &str, seed: u32, render_dist: i32) -> Self {
        Self {
            game: Game::new(cam, world_name, seed, render_dist),
            scene: Scene::new(),
            last: now_seconds(),
            input: InputController::default(),
            pointer: PointerState::default(),
            screen: AppScreen::Game,
            modifiers: Modifiers::default(),
            recenter_cursor: false,
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

    /// Update the tracked physical keyboard modifiers (Ctrl / Shift) from the
    /// platform's modifier-changed event. Independent of the rebindable
    /// Sprint/Sneak controls.
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
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

        // Route inventory clicks before reading game input, so a right-click
        // consumed by the open inventory never also fires block placement.
        if self.pointer.left_click && self.route_screen_click(screen_size, now) {
            self.pointer.left_click = false;
        }
        if self.pointer.right_click && self.route_screen_right_click(screen_size, now) {
            self.pointer.right_click = false;
        }

        let game_input = self.take_game_input();
        let events = self.game.tick(dt, &game_input);
        // Right-clicking a placed crafting table opens its 3×3 screen.
        if events.open_crafting_table && self.screen.gameplay_enabled() {
            self.open_crafting_table();
        }
        // Right-clicking a placed furnace opens its screen at that position.
        if let Some(pos) = events.open_furnace {
            if self.screen.gameplay_enabled() {
                self.open_furnace(pos);
            }
        }
        // Right-clicking a placed chest opens its screen at that position.
        if let Some(pos) = events.open_chest {
            if self.screen.gameplay_enabled() {
                self.open_chest(pos);
            }
        }
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
            // Placing a block and throwing/dropping an item both flick the hand.
            placed: events.placed_block || events.threw_item,
            dt,
        });
        // Bake the sim's per-frame world-render data (dropped items, particles,
        // chests, held-item light) into the render-side scene, then hand it off.
        self.scene.bake(&self.game);
        self.scene.upload(renderer);
        renderer.set_ui(UiFrame {
            open: self.screen.ui_open(),
            panel: self.screen.craft_kind(),
            inv: self.game.inventory(),
            craft: self.game.craft_grid().cells(),
            craft_result: self.game.craft_grid().result().copied(),
            furnace: self.game.open_furnace_view(),
            chest: self.game.open_chest_view(),
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
        if self.screen.ui_open() {
            self.close_menu();
        } else {
            self.open_inventory();
        }
    }

    fn open_inventory(&mut self) {
        self.enter_menu(AppScreen::Inventory);
        self.game.open_crafting(2);
    }

    /// Open the 3×3 crafting-table screen (after right-clicking a placed table).
    fn open_crafting_table(&mut self) {
        self.enter_menu(AppScreen::CraftingTable);
        self.game.open_crafting(3);
    }

    /// Open the furnace screen for the furnace at `pos` (after right-clicking it).
    fn open_furnace(&mut self, pos: crate::mathh::IVec3) {
        self.enter_menu(AppScreen::Furnace);
        self.game.open_furnace_screen(pos);
    }

    /// Open the chest screen for the chest at `pos` (after right-clicking it).
    fn open_chest(&mut self, pos: crate::mathh::IVec3) {
        self.enter_menu(AppScreen::Chest);
        self.game.open_chest_screen(pos);
    }

    /// Shared menu-open bookkeeping: release the pointer grab, show + recenter the
    /// cursor next tick, and clear any stale click streak so the first click
    /// can't register a phantom double.
    fn enter_menu(&mut self, screen: AppScreen) {
        self.screen = screen;
        self.pointer.grabbing = false;
        self.recenter_cursor = true;
        self.pointer.reset_click_streak();
    }

    /// Close any open menu: return crafting-grid items to the inventory, drop back
    /// to gameplay, and re-grab the pointer.
    fn close_menu(&mut self) {
        // All three are safe to call regardless of which menu was open: a furnace /
        // chest screen leaves the craft grid empty, and the inventory/table leaves no
        // open furnace or chest.
        self.game.close_crafting();
        self.game.close_furnace();
        self.game.close_chest();
        self.screen = AppScreen::Game;
        self.pointer.grabbing = true;
    }

    fn close_screen(&mut self) -> bool {
        if self.screen.ui_open() {
            self.close_menu();
            true
        } else {
            false
        }
    }

    /// Route a left-click to the open inventory. Returns whether it was consumed
    /// (i.e. the inventory was open). No-op when closed. `now` timestamps the click
    /// for double-click detection.
    fn route_screen_click(&mut self, screen: (u32, u32), now: f64) -> bool {
        if !self.screen.ui_open() {
            return false;
        }
        self.route_inventory_click(screen, PointerButton::Primary, now);
        true
    }

    /// Route a right-click to the open inventory. Returns whether it was consumed
    /// (i.e. the inventory was open) — so a closed-inventory right-click falls
    /// through to block placement. No-op when closed.
    fn route_screen_right_click(&mut self, screen: (u32, u32), now: f64) -> bool {
        if !self.screen.ui_open() {
            return false;
        }
        self.route_inventory_click(screen, PointerButton::Secondary, now);
        true
    }

    /// Apply an inventory click (caller guarantees a menu is open). Hit-test the
    /// pixel to a slot identity, then route it through the menu's single
    /// [`Game::menu_click`] entry — one path keyed on the open container, instead of
    /// a router per container type.
    ///
    /// On a slot: shift transfers (furnace tag-routes #fuel / #smeltable, chest
    /// dumps in, otherwise hotbar↔grid); right splits / drips one; left does
    /// whole-stack pick/drop/swap — except a fast second left click on the same slot
    /// while dragging a stack gathers matching items onto the cursor (the
    /// double-click `gather` verdict App owns; see [`left_click_gather`]). Off any
    /// slot but confidently OUTSIDE the panel: throw the held stack (left = all,
    /// right = one). A click on the panel art but not a slot does nothing.
    fn route_inventory_click(&mut self, screen: (u32, u32), button: PointerButton, now: f64) {
        let cursor = (self.pointer.cursor_x, self.pointer.cursor_y);
        let shift = self.modifiers.shift;
        // The open panel's own slots take priority over the inventory grid below.
        // ONE match on the menu's edit target picks the panel's per-layout
        // hit-tester (furnace role / chest index / craft cell), replacing the old
        // is_furnace()/is_chest() ladder. A miss falls through to the inventory grid.
        let slot = self.panel_slot_at(screen, cursor).or_else(|| {
            crate::render::slot_at_cursor(screen, true, cursor).map(MenuSlot::Inventory)
        });
        match slot {
            Some(slot) => {
                let gather = self.left_click_gather(slot, button, shift, now);
                self.game.menu_click(slot, button, shift, gather);
            }
            None if !crate::render::cursor_in_panel(screen, cursor) => {
                self.pointer.reset_click_streak();
                match button {
                    PointerButton::Primary => self.game.throw_cursor_stack(),
                    PointerButton::Secondary => self.game.throw_cursor_one(),
                }
            }
            None => {}
        }
    }

    /// Hit-test the open panel's OWN slots (above the inventory grid), resolving the
    /// pixel to the panel-specific slot identity. The per-layout geometry differs by
    /// container (furnace slots vs chest slots sit at different pixels), so this stays
    /// keyed on the open container — but it is the ONE place that branches on it, the
    /// single decision point that replaced App's per-container click routers.
    fn panel_slot_at(&self, screen: (u32, u32), cursor: (f32, f32)) -> Option<MenuSlot> {
        match self.game.menu().target() {
            ContainerTarget::Furnace(_) => {
                crate::render::furnace_slot_at_cursor(screen, cursor).map(MenuSlot::Furnace)
            }
            ContainerTarget::Chest(_) => {
                crate::render::chest_slot_at_cursor(screen, cursor).map(MenuSlot::Chest)
            }
            ContainerTarget::Inventory | ContainerTarget::Table => {
                crate::render::craft_slot_at_cursor(self.screen.craft_kind(), screen, cursor)
                    .map(MenuSlot::Craft)
            }
            ContainerTarget::None => None,
        }
    }

    /// Resolve a click into the double-click `gather` verdict and keep the App's
    /// click-streak in step. A streak-advancing click is a plain left-click on a
    /// gatherable slot (an inventory or chest storage slot — the only slots whose
    /// fast re-click gathers); that case registers against the streak and reports a
    /// gather when it completes a double AND the cursor holds a stack. Every other
    /// interaction (shift / right, or a furnace/craft slot) ends the streak and never
    /// gathers, so the next left-click is a fresh single click. Chest slot indices
    /// are namespaced past the inventory's (see [`CHEST_SLOT_STREAK_BASE`]) so a chest
    /// slot and an inventory slot with the same number aren't conflated.
    fn left_click_gather(
        &mut self,
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        now: f64,
    ) -> bool {
        let streak_key = match slot {
            _ if shift || button != PointerButton::Primary => None,
            MenuSlot::Inventory(i) => Some(i),
            MenuSlot::Chest(i) => Some(CHEST_SLOT_STREAK_BASE + i),
            MenuSlot::Craft(_) | MenuSlot::Furnace(_) => None,
        };
        match streak_key {
            Some(key) => self.pointer.register_left_click(key, now) && self.game.cursor_has_stack(),
            None => {
                self.pointer.reset_click_streak();
                false
            }
        }
    }
}

/// Wheel notches of travel per hotbar slot. One classic detent is `1.0`
/// (Windows' `WHEEL_DELTA` / 120, as winit normalizes it), so a notched wheel
/// still advances exactly one slot per click. Hi-res / free-spin mice (the MX
/// Master) emit fractions of a notch many times a frame; requiring a whole
/// notch per slot — and carrying the sub-slot remainder forward — couples
/// selection to how far the wheel actually turned instead of lurching a slot on
/// every micro-event.
const SCROLL_NOTCHES_PER_SLOT: f32 = 1.0;

/// A second left-click on the same inventory slot within this window counts as a
/// double-click, which gathers matching items onto the cursor instead of dropping
/// the held stack back. Matches the classic ~250 ms double-click timeout.
const DOUBLE_CLICK_SECS: f64 = 0.25;

/// Offset added to a chest storage-slot index when feeding the double-click streak,
/// so a chest slot and an inventory slot with the same number can't be conflated
/// into a phantom double-click. Larger than any inventory slot index.
const CHEST_SLOT_STREAK_BASE: usize = 1000;

impl PointerState {
    /// Register a left-click on inventory `slot` at time `now`, returning whether
    /// it completes a double-click: a second click on the SAME slot within
    /// [`DOUBLE_CLICK_SECS`] of the first. A completed double-click consumes the
    /// streak, so a third quick click starts a fresh single click.
    fn register_left_click(&mut self, slot: usize, now: f64) -> bool {
        let is_double =
            self.last_click_slot == Some(slot) && now - self.last_click_time < DOUBLE_CLICK_SECS;
        if is_double {
            self.last_click_slot = None;
        } else {
            self.last_click_slot = Some(slot);
            self.last_click_time = now;
        }
        is_double
    }

    /// Forget any in-progress click streak (after a non-pickup interaction such as
    /// a shift-move, right-click, or throw-out), so the next left-click is a fresh
    /// single click rather than a stray double.
    fn reset_click_streak(&mut self) {
        self.last_click_slot = None;
    }

    /// Whole hotbar slots to move this frame, draining the accumulator by the
    /// notches consumed and keeping the sub-slot remainder for next frame. The
    /// result is frame-rate independent: a slow, deliberate roll yields one slot
    /// per notch; a hard flick yields several; a jittery nudge under a notch
    /// yields none.
    fn take_scroll_step(&mut self) -> i32 {
        let steps = (self.scroll_delta / SCROLL_NOTCHES_PER_SLOT).trunc();
        self.scroll_delta -= steps * SCROLL_NOTCHES_PER_SLOT;
        steps as i32
    }

    fn clear_edges(&mut self) {
        self.left_click = false;
        self.right_click = false;
    }
}

fn now_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::Control;
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

    #[test]
    fn scroll_step_needs_a_full_notch() {
        let mut p = PointerState::default();
        // Sub-notch travel accumulates without moving the selection...
        p.scroll_delta = 0.4;
        assert_eq!(p.take_scroll_step(), 0);
        p.scroll_delta += 0.4;
        assert_eq!(p.take_scroll_step(), 0);
        // ...until a whole notch is reached: one step, remainder carried.
        p.scroll_delta += 0.4;
        assert_eq!(p.take_scroll_step(), 1);
        assert!((p.scroll_delta - 0.2).abs() < 1e-4);
    }

    #[test]
    fn scroll_step_is_proportional_and_signed() {
        let mut p = PointerState::default();
        p.scroll_delta = 3.0;
        assert_eq!(p.take_scroll_step(), 3);
        assert_eq!(p.scroll_delta, 0.0);

        p.scroll_delta = -2.5;
        assert_eq!(p.take_scroll_step(), -2);
        assert!((p.scroll_delta + 0.5).abs() < 1e-4);
    }

    #[test]
    fn scroll_step_carries_remainder_across_frames() {
        // A free-spin wheel emits a stream of tiny deltas; the slot must advance
        // once per accumulated notch, not once per micro-event.
        let mut p = PointerState::default();
        let mut steps = 0;
        for _ in 0..25 {
            p.scroll_delta += 0.1;
            steps += p.take_scroll_step();
        }
        assert_eq!(steps, 2); // 25 * 0.1 = 2.5 notches -> 2 whole slots
        assert!((p.scroll_delta - 0.5).abs() < 1e-4);
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

    fn cursor_over_craft(
        screen: (u32, u32),
        kind: crate::render::CraftKind,
        hit: crate::render::CraftHit,
    ) -> (f32, f32) {
        for y in 0..screen.1 {
            for x in 0..screen.0 {
                let c = (x as f32 + 0.5, y as f32 + 0.5);
                if crate::render::craft_slot_at_cursor(kind, screen, c) == Some(hit) {
                    return c;
                }
            }
        }
        panic!("no cursor position maps to craft {hit:?}");
    }

    #[test]
    fn craft_slot_clicks_route_through_to_crafting() {
        use crate::render::{CraftHit, CraftKind};
        let mut app = app();
        // Give the player one oak log and open the inventory (2×2 crafting).
        app.game
            .add_to_inventory(ItemStack::new(ItemType::OakLog, 1));
        app.handle_control(Control::ToggleInventory, true);
        let screen = (1280u32, 720u32);

        // Pick the log up from inventory slot 0.
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);
        app.route_screen_click(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());

        // Drop it into the first 2×2 craft input cell -> planks preview appears.
        let cc = cursor_over_craft(screen, CraftKind::Inventory, CraftHit::Input(0));
        app.set_cursor_position(cc.0, cc.1);
        app.route_screen_click(screen, 0.1);
        assert!(
            app.game.inventory().cursor().is_none(),
            "log placed into the craft cell"
        );
        assert_eq!(
            app.game.craft_grid().result().map(|s| s.item),
            Some(ItemType::OakPlanks)
        );

        // Click the result slot: 4 planks land on the cursor, ingredients consumed.
        let rc = cursor_over_craft(screen, CraftKind::Inventory, CraftHit::Result);
        app.set_cursor_position(rc.0, rc.1);
        app.route_screen_click(screen, 0.2);
        assert_eq!(
            app.game.inventory().cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        assert!(app.game.craft_grid().result().is_none());
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
        app.route_screen_click(screen, 0.0);
        let cc = cursor_over_craft(
            screen,
            crate::render::CraftKind::Inventory,
            crate::render::CraftHit::Input(0),
        );
        app.set_cursor_position(cc.0, cc.1);
        app.route_screen_click(screen, 0.1);
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
    fn route_inventory_click_open_picks_up_slot_stack() {
        let mut app = app_with_grass();
        app.handle_control(Control::ToggleInventory, true);
        assert!(app.inventory_open());
        let screen = (1280, 720);
        let (cx, cy) = cursor_over_slot(screen, 0);
        app.set_cursor_position(cx, cy);

        assert!(app.game.inventory().cursor().is_none());
        let item0 = app.game.inventory().slot(0).unwrap().item;

        let consumed = app.route_screen_click(screen, 0.0);
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
        app.route_screen_click(screen, 0.0);
        app.route_screen_click(screen, 0.1);
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
        app.route_screen_click(screen, 0.0);
        app.route_screen_click(screen, 1.0);
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
        app.route_screen_click(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());

        // A fast click on a DIFFERENT slot is a normal drop, not a gather: the held
        // stack lands in the first (empty) main-grid slot.
        let dest = crate::inventory::HOTBAR_LEN;
        let (dx, dy) = cursor_over_slot(screen, dest);
        app.set_cursor_position(dx, dy);
        app.route_screen_click(screen, 0.05);
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
        let consumed = app.route_screen_click((1280, 720), 0.0);
        assert!(!consumed);
        assert!(app.game.inventory().cursor().is_none());
        assert_eq!(app.game.inventory().slot(0).map(|s| s.count), before);
    }

    #[test]
    fn q_drops_one_held_item_while_playing() {
        let mut app = app_with_grass();
        let before = app.game.inventory().selected().unwrap().count;
        app.handle_control(Control::DropItem, true);
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
        let consumed = app.route_screen_right_click(screen, 0.0);
        assert!(consumed);
        assert_eq!(app.game.inventory().cursor().unwrap().count, 32);
        assert_eq!(app.game.inventory().slot(0).unwrap().count, 32);
    }

    #[test]
    fn route_inventory_right_click_closed_falls_through_to_placement() {
        // Closed inventory: a right-click is NOT consumed, so it can place a block.
        let mut app = app();
        assert!(!app.inventory_open());
        assert!(!app.route_screen_right_click((1280, 720), 0.0));
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
        app.route_screen_click(screen, 0.0);
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
        app.route_screen_click(screen, 0.0);
        assert!(app.game.inventory().cursor().is_some());
        // Click the top-left corner: confidently outside the inventory panel.
        app.set_cursor_position(0.0, 0.0);
        app.route_screen_click(screen, 0.1);
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
        app.route_screen_click(screen, 0.0); // pick up the stack
        assert!(app.game.inventory().cursor().is_some());
        // A point inside the panel but on no slot: the held stack is kept.
        let inside_panel_gap = panel_gap_point(screen);
        app.set_cursor_position(inside_panel_gap.0, inside_panel_gap.1);
        app.route_screen_click(screen, 0.1);
        assert!(
            app.game.inventory().cursor().is_some(),
            "click on panel art must not throw the stack"
        );
    }

    /// A point inside the panel rectangle that is NOT over any slot.
    fn panel_gap_point(screen: (u32, u32)) -> (f32, f32) {
        for y in 0..screen.1 {
            for x in 0..screen.0 {
                let c = (x as f32 + 0.5, y as f32 + 0.5);
                if crate::render::cursor_in_panel(screen, c)
                    && crate::render::slot_at_cursor(screen, true, c).is_none()
                {
                    return c;
                }
            }
        }
        panic!("no in-panel, off-slot point found");
    }
}
