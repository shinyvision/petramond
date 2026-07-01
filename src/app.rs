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

use screen::AppScreen;
pub use screen::CursorPolicy;

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
    /// Player health at the last render, so a change (fall damage) forces a redraw even
    /// when the view is otherwise idle — the hearts settle a tick or two after landing,
    /// once the camera has already stopped moving.
    last_health: Option<crate::gui::HealthView>,
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
            last_health: None,
            hand: HandTriggers::default(),
        }
    }

    /// Flush the world to disk on quit. The `WorldSave` I/O thread is joined when
    /// the `App` (and the `World` it owns) drops, after this queues the writes.
    pub fn save_on_exit(&mut self) {
        self.game.save_all();
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
mod tests;
