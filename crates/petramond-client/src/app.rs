//! Application shell for the native desktop host.
//!
//! The app owns window-level state: current screen, input aggregation, cursor
//! policy, frame time, and renderer handoff. The voxel demo itself lives in
//! `game`, and first-person hand animation lives in the renderer presentation
//! layer.

mod chat;
mod client_audio;
mod client_mod_ui;
mod connect;
mod crafting_browser;
mod creative;
mod gui_router;
mod gui_value;
mod heap_reclaim;
mod hotbar_notice;
mod hud_fx;
mod input;
mod inventory_menu;
mod item_tooltip;
mod menu_lifecycle;
mod music;
mod options;
mod options_state;
mod pointer;
mod presentation_events;
mod render;
mod schematic_library;
mod screen;
mod session_ui;
mod shell;
mod shell_docs;
mod shell_state;
mod ui_runtime;
mod ui_snapshot;
mod update;

use screen::AppScreen;
pub use screen::{CursorIcon, CursorPolicy};

use crate::app::gui_router::GuiRouter;
use crate::app::input::{ControlEvent, Controls};
use crate::game::presentation::GamePresentationScratch;
use crate::game::Game;
use petramond_input::controls::{BindableAction, Control, Modifiers};
use petramond_render::camera::Camera;
use petramond_render::Scene;

pub struct App {
    game: Option<Game>,
    /// App-side state scoped to the current game session (HUD notice, the
    /// creative / schematic forms, LAN status), replaced wholesale when a
    /// session starts or ends.
    session_ui: session_ui::SessionUi,
    shell_camera: Camera,
    render_dist: i32,
    /// Reusable builder for neutral per-frame presentation data read from the game.
    presentation: GamePresentationScratch,
    /// Render-side translation of neutral per-frame presentation data into the
    /// renderer's wire structs.
    scene: Scene,
    /// Sound orchestration: the audio engine, the soundtrack, and the
    /// client-owned cue/cadence bookkeeping between game events and plays.
    sound: client_audio::ClientAudio,
    last: f64,
    /// Raw input resolution: held controls, pointer, modifiers, the action
    /// table and the held bindings.
    controls: Controls,
    gui_router: GuiRouter,
    /// GUI-document runtime driver (every screen is document-backed).
    ui: ui_runtime::AppUi,
    /// Search/selection and filtered rows for player crafting. Presentation
    /// state only; the stable selected key is sent on CRAFT.
    crafting_browser: crafting_browser::CraftingBrowser,
    /// Reused draw list for the active GUI document.
    composed_doc: petramond_ui::DrawList,
    composed_doc_images: Vec<petramond::gui::DocImageSource>,
    /// Open client-WASM physical-pixel canvas, separate from GUI documents.
    client_canvas: Option<client_mod_ui::ClientCanvasState>,
    /// Reused renderer handoff for always-on client overlays plus the canvas.
    client_overlay_images: Vec<petramond_render::ClientOverlayImage>,
    chat: chat::ChatUi,
    screen: AppScreen,
    /// The Options flow: persistent settings (`client.json`), slider
    /// previews, the armed control remap, the renderer refresh flag.
    options: options_state::OptionsState,
    /// `now_seconds` of the last [`render`](Self::render), so the held-item animation
    /// advances by draw time even when the platform coalesces or skips a redraw.
    last_render: f64,
    /// Short-lived HUD/hand presentation: hurt shake, sleep-overlay hand,
    /// heart wiggle, and the local rigs' graph events awaiting the next draw.
    hud_fx: hud_fx::HudFx,
    /// Returns the allocator's free pages to the OS once terrain settles (see
    /// [`heap_reclaim`]).
    heap_reclaim: heap_reclaim::IdleHeapReclaim,
    /// The title flow's state: world list and selection, the open page's
    /// session, the connect session, the last disconnect reason.
    shell: shell_state::ShellState,
    /// The last session's section cache, harvested at teardown: the next
    /// remote join claims it in its Join manifest so a reconnect re-promotes
    /// cached terrain instead of re-streaming it.
    retained_section_cache: Option<crate::game::section_cache::SectionCache>,
    quit_requested: bool,
    renderer_world_clear_pending: bool,
}

impl App {
    pub fn new(cam: Camera, render_dist: i32) -> Self {
        // The file is the persistence layer for options; the render_dist PARAM
        // stays authoritative for this run (the host resolved env > file), so
        // mirror it back — the Options slider shows and stores the live value.
        // Tests run on defaults: the suite must never read (or later rewrite)
        // the developer's real client.json — same rule as `persist_identity`.
        let mut settings = if cfg!(test) {
            petramond::save::client::ClientSettings::default()
        } else {
            petramond::save::client::load()
        };
        settings.render_dist = render_dist;
        let sound = client_audio::ClientAudio::new(
            settings.master_volume,
            settings.sound_volume,
            settings.music_volume,
        );
        let mut app = Self {
            game: None,
            session_ui: Default::default(),
            shell_camera: cam,
            render_dist,
            presentation: GamePresentationScratch::new(),
            scene: Scene::new(),
            sound,
            last: now_seconds(),
            controls: Controls::new(),
            gui_router: GuiRouter::default(),
            ui: ui_runtime::AppUi::new(),
            crafting_browser: Default::default(),
            composed_doc: petramond_ui::DrawList::default(),
            composed_doc_images: Vec::new(),
            client_canvas: None,
            client_overlay_images: Vec::new(),
            chat: chat::ChatUi::default(),
            screen: AppScreen::Title,
            options: options_state::OptionsState::new(settings),
            last_render: now_seconds(),
            hud_fx: Default::default(),
            heap_reclaim: Default::default(),
            shell: Default::default(),
            retained_section_cache: None,
            quit_requested: false,
            renderer_world_clear_pending: true,
        };
        app.controls.pointer.release_for_menu();
        app.shell.refresh_worlds();
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
        let Some(event) = self.controls.input.set_control(control, down) else {
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
                    self.controls.pointer.release_for_menu();
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
                if self.game.is_some() && (self.screen.gameplay_enabled() || self.screen.ui_open())
                {
                    self.toggle_inventory();
                }
                true
            }
            ControlEvent::ToggleCreative
            | ControlEvent::UndoEdit
            | ControlEvent::RedoEdit
            | ControlEvent::JumpPressed => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        match event {
                            ControlEvent::ToggleCreative => game.toggle_creative_mode(),
                            ControlEvent::UndoEdit => game.undo_edit(),
                            ControlEvent::RedoEdit => game.redo_edit(),
                            _ => game.jump_pressed(now_seconds()),
                        }
                    }
                }
                true
            }
            ControlEvent::AdjustTool(steps) => {
                // With nothing to adjust, the wheel is the hotbar's whatever
                // is held with it (sprinting on Ctrl scrolls slots as ever).
                let taken = self.screen.gameplay_enabled()
                    && self
                        .game
                        .as_mut()
                        .is_some_and(|game| game.adjust_tool(steps));
                if !taken {
                    self.controls.input.step_hotbar(self.hotbar_step_for_adjust(steps));
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
            // Attack / Interact are rebindable: whatever raw input fired them
            // (mouse button by default, any key/scroll after a remap) lands in
            // the same pointer break/use state gameplay consumes. Downs only
            // count in gameplay; releases always land so nothing sticks held.
            ControlEvent::Attack { down } => {
                if self.screen.gameplay_enabled() || !down {
                    self.controls.pointer.set_gameplay_button(
                        petramond_world::gui_state::PointerButton::Primary,
                        down,
                    );
                }
                true
            }
            ControlEvent::Interact { down } => {
                if self.screen.gameplay_enabled() || !down {
                    self.controls.pointer.set_gameplay_button(
                        petramond_world::gui_state::PointerButton::Secondary,
                        down,
                    );
                }
                true
            }
            ControlEvent::SelectHotbar(slot) => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.set_active_hotbar(slot);
                    }
                }
                true
            }
            ControlEvent::DropItem => {
                if self.screen.ui_open() {
                    let (x, y) = self.controls.pointer.cursor();
                    if let (Some(slot), Some(game)) =
                        (self.ui.menu_slot_at(x, y), self.game.as_mut())
                    {
                        // Menu shortcuts deliberately use the physical Ctrl
                        // modifier; movement bindings do not redefine GUI
                        // conventions.
                        game.menu_drop(slot, self.controls.modifiers.ctrl);
                    }
                } else if self.screen.gameplay_enabled() {
                    // In captured gameplay, holding the SPRINT control
                    // (wherever it is bound) drops the selected whole stack.
                    let whole_stack = self.controls.input.sprint_held();
                    if let Some(game) = self.game.as_mut() {
                        game.drop_selected_item(whole_stack);
                    }
                }
                true
            }
            ControlEvent::SwapOffHand => {
                // In a menu, F swaps the off-hand with the HOVERED slot (the
                // Drop Item shape: hit-test the solved document cells); over
                // no slot it does nothing. In gameplay it swaps with the
                // selected hotbar slot. Focused text inputs already swallowed
                // the press upstream (`handle_raw_key`).
                if self.screen.ui_open() {
                    let (x, y) = self.controls.pointer.cursor();
                    if let (Some(slot), Some(game)) =
                        (self.ui.menu_slot_at(x, y), self.game.as_mut())
                    {
                        game.menu_swap_off_hand(slot);
                    }
                } else if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        game.swap_off_hand();
                    }
                }
                true
            }
            ControlEvent::RotateHeldBlock => {
                if self.screen.gameplay_enabled() {
                    if let Some(game) = self.game.as_mut() {
                        if !game.rotate_schematic_preview() {
                            game.toggle_held_block_rotation();
                        }
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

    /// Update the tracked physical keyboard modifiers (Ctrl / Shift / Alt /
    /// Meta) from the platform's modifier-changed event. Independent of the
    /// rebindable controls — but a lifted modifier releases any held binding
    /// CHORD that required it (Ctrl+B sprint stops when Ctrl lifts).
    /// The hotbar step a tool-adjust that found nothing to adjust should make.
    ///
    /// It is decided by the HOTBAR's own bindings, never by the tool step's
    /// sign: the two pairs are bound independently, and a player who binds
    /// tool adjust the opposite way round from the wheel (scroll up raises)
    /// must not have their hotbar run backwards whenever the chord's modifier
    /// happens to be held — which sprint's default Ctrl means is *while
    /// sprinting*. So the input is asked what it means to the hotbar. An
    /// adjust bound to something the hotbar does not share (a key) keeps its
    /// own step; there is no wheel convention to defer to.
    fn hotbar_step_for_adjust(&self, steps: i32) -> i32 {
        let input = self
            .settings
            .bindings
            .binding(if steps >= 0 {
                BindableAction::AdjustToolNext
            } else {
                BindableAction::AdjustToolPrev
            })
            .input;
        for (action, step) in [
            (BindableAction::HotbarNext, 1),
            (BindableAction::HotbarPrev, -1),
        ] {
            if self.options.settings.bindings.binding(action).input == input {
                return step;
            }
        }
        steps
    }

    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.controls.modifiers = modifiers;
        let mut out = Vec::new();
        self.controls.binding_engine
            .on_modifiers_changed(modifiers, &mut out);
        self.dispatch_actions(out);
    }

    /// Which GUI document backs the current screen, if any. Document-backed
    /// screens draw + route input through the petramond-ui runtime; a screen with
    /// no loaded document draws (and routes) nothing.
    pub fn doc_ui_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        use petramond_world::gui_state::GuiKind;
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
            AppScreen::Options => GuiKind::Options,
            AppScreen::OptionsSound => GuiKind::OptionsSound,
            AppScreen::OptionsControls => GuiKind::OptionsControls,
            AppScreen::OptionsGraphics => GuiKind::OptionsGraphics,
            AppScreen::Pause => GuiKind::Pause,
            AppScreen::Sleeping => GuiKind::Sleep,
            AppScreen::Dead => GuiKind::Death,
            AppScreen::Menu(kind) => kind,
            AppScreen::ClientModGui(kind) => kind,
            AppScreen::Schematics => GuiKind::Schematics,
            _ => return None,
        };
        ui_runtime::AppUi::doc_backed(kind).then_some(kind)
    }

    /// The subset of [`doc_ui_kind`](Self::doc_ui_kind) where the whole frame
    /// belongs to the shell (no game simulation behind it). Game menus (mod
    /// GUIs, containers) return `None` here — they drive their document UI
    /// AND tick the game.
    pub fn doc_shell_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        if self.screen.ui_open() || self.screen.client_ui_open() || self.screen.overlay_open() {
            return None;
        }
        self.doc_ui_kind()
    }

    /// The subset of [`doc_ui_kind`](Self::doc_ui_kind) for gameplay OVERLAY
    /// screens (sleep / death): the document owns input like a shell screen —
    /// its events dispatch to a controller, not to slot routing — but the
    /// simulation keeps ticking underneath (the sleep timer and respawn are
    /// tick-owned).
    pub fn doc_overlay_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        if !self.screen.overlay_open() {
            return None;
        }
        self.doc_ui_kind()
    }

    /// Whether the hotbar HUD draws from its GUI document this frame
    /// (gameplay screen only; presentation-only, input stays with the game).
    pub fn doc_hud_active(&self) -> bool {
        matches!(self.screen, AppScreen::Game | AppScreen::Chat)
            && self.game.is_some()
            && ui_runtime::AppUi::doc_backed(petramond_world::gui_state::GuiKind::Hotbar)
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
