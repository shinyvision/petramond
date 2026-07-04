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
mod shell;
mod text_input;
mod ui_snapshot;
mod update;

use screen::AppScreen;
pub use screen::{CursorIcon, CursorPolicy};
pub(crate) use text_input::TextClipboard;

use crate::app::gui_router::GuiRouter;
use crate::app::input::{ControlEvent, InputController};
use crate::app::pointer::PointerState;
use crate::app::text_input::TextInput;
use crate::audio::Audio;
use crate::camera::Camera;
use crate::controls::{Control, Modifiers};
use crate::game::presentation::GamePresentationScratch;
use crate::game::Game;
use crate::render::Scene;

pub struct App {
    game: Option<Game>,
    shell_camera: Camera,
    render_dist: i32,
    /// Reusable builder for neutral per-frame presentation data read from the game.
    presentation: GamePresentationScratch,
    /// Render-side translation of neutral per-frame presentation data into the
    /// renderer's wire structs.
    scene: Scene,
    /// Spatial sound commands emitted by ticks since the last render. They are
    /// applied alongside the same mob presentation snapshot the renderer uses.
    spatial_sound_commands: Vec<crate::game::ModSpatialSoundCommand>,
    spatial_mob_positions: Vec<(u64, crate::mathh::Vec3)>,
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
    /// `now_seconds` of the last [`render`](Self::render), so the held-item animation
    /// advances by draw time even when the platform coalesces or skips a redraw.
    last_render: f64,
    /// First-person hand-animation triggers latched since the last render, so a
    /// swing/place/break begun on an un-drawn update isn't lost before the next draw.
    hand: HandTriggers,
    worlds: Vec<crate::save::WorldInfo>,
    selected_world: Option<usize>,
    world_scroll: usize,
    /// The World Settings session for the selected world (`None` unless the
    /// screen is open): installed pack rows + the world's disabled set.
    world_settings: Option<shell::WorldSettingsSession>,
    create_world_name: TextInput,
    create_world_seed: TextInput,
    focused_create_field: Option<shell::CreateField>,
    dragged_create_field: Option<(shell::CreateField, usize)>,
    shell_clicks: shell::ShellClickStreak,
    quit_requested: bool,
    renderer_world_clear_pending: bool,
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
    pub fn new(cam: Camera, render_dist: i32) -> Self {
        let mut app = Self {
            game: None,
            shell_camera: cam,
            render_dist,
            presentation: GamePresentationScratch::new(),
            scene: Scene::new(),
            spatial_sound_commands: Vec::new(),
            spatial_mob_positions: Vec::new(),
            audio: Audio::new(),
            last: now_seconds(),
            input: InputController::default(),
            pointer: PointerState::default(),
            gui_router: GuiRouter::default(),
            screen: AppScreen::Title,
            modifiers: Modifiers::default(),
            last_render: now_seconds(),
            hand: HandTriggers::default(),
            worlds: Vec::new(),
            selected_world: None,
            world_scroll: 0,
            world_settings: None,
            create_world_name: TextInput::new(48),
            create_world_seed: TextInput::new(48),
            focused_create_field: None,
            dragged_create_field: None,
            shell_clicks: shell::ShellClickStreak::default(),
            quit_requested: false,
            renderer_world_clear_pending: true,
        };
        app.pointer.release_for_menu();
        app.refresh_worlds();
        app
    }

    #[cfg(test)]
    pub(crate) fn new_in_game(cam: Camera, world_name: &str, seed: u32, render_dist: i32) -> Self {
        let mut app = Self::new(cam, render_dist);
        app.start_game(world_name, seed);
        app
    }

    /// Flush the world to disk on quit. The `WorldSave` I/O thread is joined when
    /// the `App` (and the `World` it owns) drops, after this queues the writes.
    pub fn save_on_exit(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.save_all();
        }
    }

    #[inline]
    pub fn cursor_policy(&self, screen_size: (u32, u32)) -> CursorPolicy {
        let mut policy = CursorPolicy::for_screen(self.screen);
        if policy.visible && self.shell_text_cursor_hovered(screen_size) {
            policy.icon = CursorIcon::Text;
        }
        policy
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let aspect = width as f32 / height.max(1) as f32;
        self.shell_camera.aspect = aspect;
        if let Some(game) = self.game.as_mut() {
            game.set_aspect(aspect);
        }
    }

    /// Apply a shared control event. Returns false only when the app did not
    /// consume the control, e.g. Escape with no screen open on native.
    pub fn handle_control(&mut self, control: Control, down: bool) -> bool {
        let Some(event) = self.input.set_control(control, down) else {
            return true;
        };

        match event {
            ControlEvent::ToggleInventory => {
                if self.game.is_some() && !self.screen.shell_open() {
                    self.toggle_inventory();
                }
                true
            }
            ControlEvent::TogglePlayerMode => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.toggle_player_mode();
                    }
                }
                true
            }
            ControlEvent::CloseScreen => self.close_screen(),
            ControlEvent::SelectHotbar(slot) => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.set_active_hotbar(slot);
                    }
                }
                true
            }
            ControlEvent::DropItem => {
                // Q drops the held item only while playing (not in a menu). The
                // physical Ctrl modifier (not the sprint key) selects whole-stack.
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.drop_selected_item(self.modifiers.ctrl);
                    }
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
