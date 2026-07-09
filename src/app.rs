//! Application shell for the native desktop host.
//!
//! The app owns window-level state: current screen, input aggregation, cursor
//! policy, frame time, and renderer handoff. The voxel demo itself lives in
//! `game`, and first-person hand animation lives in the renderer presentation
//! layer.

mod chat;
mod connect;
mod gui_router;
mod input;
mod menu_lifecycle;
mod pointer;
mod presentation_events;
mod render;
mod screen;
mod shell;
mod shell_docs;
mod ui_runtime;
mod ui_snapshot;
mod update;

use std::collections::HashMap;

use screen::AppScreen;
pub use screen::{CursorIcon, CursorPolicy};

use crate::app::gui_router::GuiRouter;
use crate::app::input::{ControlEvent, InputController};
use crate::app::pointer::PointerState;
use crate::audio::Audio;
use crate::camera::Camera;
use crate::controls::{Control, Modifiers};
use crate::game::presentation::GamePresentationScratch;
use crate::game::Game;
use crate::render::Scene;

const MOB_SOUND_HANDLE_START: u64 = 1 << 63;

/// How long the hurt screen/hand shake (and red edge flash) lasts. Punchy and
/// short: an unmistakable "get out of here", not a lasting wobble.
const HURT_SHAKE_SECS: f32 = 0.25;
/// How long the hand remains visible after a bed click that opens the sleep
/// overlay, giving the interact jab time to read before the sleeping view takes over.
const SLEEP_INTERACT_HAND_SECS: f32 = 0.30;

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
    /// Gameplay-originated mob sound events waiting for the next presentation
    /// snapshot, where they can be pinned to interpolated mob positions.
    mob_sound_events: Vec<crate::game::MobSoundEvent>,
    /// Positional world-event one-shots (block place/break, doors, chest
    /// lids, foreign pickups) waiting for the next render's spatial listener.
    world_sound_cues: Vec<(crate::audio::Sound, crate::mathh::Vec3)>,
    /// Client-owned idle sound scheduling per live mob session id.
    mob_sound_state: HashMap<u64, MobSoundState>,
    next_mob_sound_handle: u64,
    /// Client-side sound engine. Drains the sim's per-tick [`crate::audio::SoundEvent`]s
    /// each frame and plays them; never part of the deterministic simulation.
    audio: Audio,
    last: f64,
    input: InputController,
    pointer: PointerState,
    gui_router: GuiRouter,
    /// GUI-document runtime driver (every screen is document-backed).
    ui: ui_runtime::AppUi,
    chat: chat::ChatUi,
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
    /// Seconds left of the hurt screen/hand shake, latched to
    /// [`HURT_SHAKE_SECS`] when a `player_damaged` event arrives and decayed by
    /// render time. Presentation-only.
    hurt_shake_t: f32,
    /// Seconds left to keep the hand visible over the sleep overlay after the
    /// bed interaction jab starts. Presentation-only.
    sleep_interact_hand_t: f32,
    /// The HUD health drawn last frame, for change detection: any difference
    /// starts a heart wiggle. `None` while no bar is drawn (shell/spectator),
    /// so re-entering never wiggles from a stale comparison. Presentation-only.
    prev_heart_health: Option<i32>,
    /// The active heart-wiggle burst: the CHANGED half-heart range plus its
    /// wall-clock start (the 200 ms window is real time, not ticks — a paused
    /// or slowed sim must not stretch it). Presentation-only.
    heart_wiggle: Option<HeartWiggle>,
    worlds: Vec<crate::save::WorldInfo>,
    selected_world: Option<usize>,
    /// The World Settings session for the selected world (`None` unless the
    /// screen is open): installed pack rows + the world's disabled set.
    world_settings: Option<shell::WorldSettingsSession>,
    /// The Connect to Server session: entry fields, the off-thread connect
    /// worker's channel, and the mods a refused join reported missing.
    connect: connect::ConnectSession,
    /// The port the running HOST session is open to LAN on (`None` = not
    /// open). Drives the pause menu's Open to LAN button/label.
    lan_port: Option<u16>,
    /// The last Open to LAN failure, shown inline on the pause menu; cleared
    /// when the pause screen closes.
    lan_error: Option<String>,
    /// Why the last session ended, shown by the Disconnected screen.
    disconnect_message: String,
    quit_requested: bool,
    renderer_world_clear_pending: bool,
}

/// One heart-wiggle burst: hearts overlapping `[lo, hi)` (half-heart points —
/// the points gained by a heal or lost to a hit) shake for
/// [`HEART_WIGGLE_SECS`] of wall-clock time from `started`.
#[derive(Copy, Clone)]
struct HeartWiggle {
    lo: i32,
    hi: i32,
    started: f64,
}

/// How long a changed heart wiggles, in REAL seconds (per design: not ticks).
const HEART_WIGGLE_SECS: f64 = 0.2;

/// One-shot first-person hand-animation triggers, latched by [`App::update`] and
/// consumed by the next [`App::render`]. OR-merged across updates so none is dropped
/// when several sim updates run between two draws.
#[derive(Default, Copy, Clone)]
struct HandTriggers {
    broke: bool,
    placed: bool,
    swung: bool,
}

#[derive(Default)]
struct MobSoundState {
    next_idle_tick: u64,
    sequence: u64,
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
            mob_sound_events: Vec::new(),
            world_sound_cues: Vec::new(),
            mob_sound_state: HashMap::new(),
            next_mob_sound_handle: MOB_SOUND_HANDLE_START,
            audio: Audio::new(),
            last: now_seconds(),
            input: InputController::default(),
            pointer: PointerState::default(),
            gui_router: GuiRouter::default(),
            ui: ui_runtime::AppUi::new(),
            chat: chat::ChatUi::default(),
            screen: AppScreen::Title,
            modifiers: Modifiers::default(),
            last_render: now_seconds(),
            hand: HandTriggers::default(),
            hurt_shake_t: 0.0,
            sleep_interact_hand_t: 0.0,
            prev_heart_health: None,
            heart_wiggle: None,
            worlds: Vec::new(),
            selected_world: None,
            world_settings: None,
            connect: connect::ConnectSession::default(),
            lan_port: None,
            lan_error: None,
            disconnect_message: String::new(),
            quit_requested: false,
            renderer_world_clear_pending: true,
        };
        app.pointer.release_for_menu();
        app.refresh_worlds();
        app
    }

    /// Flush the world to disk on quit: a save request to the server thread.
    /// Dropping the `App` (→ `Game` → `ServerHandle`) then shuts the server
    /// down, which saves again and joins — the request here just bounds the
    /// window if teardown is interrupted.
    pub fn save_on_exit(&mut self) {
        if let Some(game) = self.game.as_mut() {
            game.save_all();
        }
    }

    // Known polish gap: no Text (I-beam) cursor over document text inputs yet;
    // every visible-cursor screen uses the default arrow.
    #[inline]
    pub fn cursor_policy(&self) -> CursorPolicy {
        CursorPolicy::for_screen(self.screen)
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
            ControlEvent::OpenChat { command } => {
                if self.screen == AppScreen::Game && self.game.is_some() {
                    self.screen = AppScreen::Chat;
                    let now = now_seconds();
                    self.chat.clear_draft(now);
                    if command {
                        self.chat.insert_text("/", now);
                    }
                    self.pointer.release_for_menu();
                }
                true
            }
            // Chat owns the keyboard: swallow other gameplay controls so new
            // bindings cannot silently fire while the draft is open.
            _ if self.screen == AppScreen::Chat => match event {
                ControlEvent::CloseScreen => self.close_screen(),
                _ => true,
            },
            ControlEvent::ToggleInventory => {
                // Not from a shell screen, and not over the sleep/death
                // overlays — an inventory opened over a running sleep would
                // strand the overlay's tick-owned state behind another screen.
                if self.game.is_some() && !self.screen.shell_open() && !self.screen.overlay_open() {
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
            ControlEvent::RotateHeldBlock => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.toggle_held_block_rotation();
                    }
                }
                true
            }
            ControlEvent::TogglePerspective => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.toggle_third_person();
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

    /// Which GUI document backs the current screen, if any. Document-backed
    /// screens draw + route input through the petramond-ui runtime; a screen with
    /// no loaded document draws (and routes) nothing.
    pub(crate) fn doc_ui_kind(&self) -> Option<crate::gui::GuiKind> {
        use crate::gui::GuiKind;
        let kind = match self.screen {
            AppScreen::Title if std::env::var_os("PETRAMOND_UI_DEMO").is_some() => GuiKind::Demo,
            AppScreen::Title => GuiKind::Title,
            AppScreen::WorldSelect => GuiKind::WorldSelect,
            AppScreen::WorldSettings => GuiKind::WorldSettings,
            AppScreen::CreateWorld => GuiKind::CreateWorld,
            AppScreen::DeleteWorld => GuiKind::DeleteWorld,
            AppScreen::ConnectServer => GuiKind::ConnectServer,
            AppScreen::ModsMissing => GuiKind::ModsMissing,
            AppScreen::ConnectionLost => GuiKind::ConnectionLost,
            AppScreen::Pause => GuiKind::Pause,
            AppScreen::Sleeping => GuiKind::Sleep,
            AppScreen::Dead => GuiKind::Death,
            AppScreen::ModGui(kind) => kind,
            AppScreen::Inventory => GuiKind::Inventory,
            AppScreen::CraftingTable => GuiKind::CraftingTable,
            AppScreen::Furnace => GuiKind::Furnace,
            AppScreen::Chest => GuiKind::Chest,
            AppScreen::FurnitureWorkbench => GuiKind::FurnitureWorkbench,
            _ => return None,
        };
        ui_runtime::AppUi::doc_backed(kind).then_some(kind)
    }

    /// The subset of [`doc_ui_kind`](Self::doc_ui_kind) where the whole frame
    /// belongs to the shell (no game simulation behind it). Game menus (mod
    /// GUIs, containers) return `None` here — they drive their document UI
    /// AND tick the game.
    pub(crate) fn doc_shell_kind(&self) -> Option<crate::gui::GuiKind> {
        if self.screen.ui_open() || self.screen.overlay_open() {
            return None;
        }
        self.doc_ui_kind()
    }

    /// The subset of [`doc_ui_kind`](Self::doc_ui_kind) for gameplay OVERLAY
    /// screens (sleep / death): the document owns input like a shell screen —
    /// its events dispatch to a controller, not to slot routing — but the
    /// simulation keeps ticking underneath (the sleep timer and respawn are
    /// tick-owned).
    pub(crate) fn doc_overlay_kind(&self) -> Option<crate::gui::GuiKind> {
        if !self.screen.overlay_open() {
            return None;
        }
        self.doc_ui_kind()
    }

    /// Whether the hotbar HUD draws from its GUI document this frame
    /// (gameplay screen only; presentation-only, input stays with the game).
    pub(crate) fn doc_hud_active(&self) -> bool {
        matches!(self.screen, AppScreen::Game | AppScreen::Chat)
            && self.game.is_some()
            && ui_runtime::AppUi::doc_backed(crate::gui::GuiKind::Hotbar)
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
