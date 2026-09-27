//! Application shell for the native desktop host.
//!
//! The app owns window-level state: current screen, input aggregation, cursor
//! policy, frame time, and renderer handoff. The voxel demo itself lives in
//! `game`, and first-person hand animation lives in the renderer presentation
//! layer.

mod account;
mod chat;
mod client_audio;
pub(crate) mod client_doc_events;
mod client_mod_ui;
mod connect;
mod content;
mod crafting_browser;
mod creative;
mod exit;
mod frame_size;
mod frame_ui;
mod gui_router;
mod gui_value;
mod heap_reclaim;
mod hotbar_notice;
mod hud_fx;
mod input;
mod inventory_menu;
mod item_tooltip;
mod launched;
mod media;
mod menu_lifecycle;
mod music;
mod options;
mod options_state;
mod pointer;
mod presentation_events;
mod render;
mod schematic_library;
mod screen;
mod screen_flow;
mod session;
mod shell;
mod shell_docs;
mod shell_state;
mod ui_runtime;
mod ui_snapshot;
mod update;
mod world_marks;

pub use exit::ExitKind;
use screen::AppScreen;
pub use screen::{CursorIcon, CursorPolicy};

use crate::app::gui_router::GuiRouter;
use crate::app::input::{ControlEvent, Controls};
use crate::app::session::Session;
use petramond_input::controls::{BindableAction, Control, Modifiers};
use petramond_render::camera::Camera;

pub struct App {
    /// The live game session and everything scoped to it (see
    /// [`session::Session`]): starting a session constructs it, ending one
    /// drops it.
    session: Option<Session>,
    shell_camera: Camera,
    render_dist: i32,
    /// Sound orchestration's app-lifetime half: the audio engine and the
    /// soundtrack. The session's cue/cadence bookkeeping lives in the session.
    sound: client_audio::ClientAudio,
    last: f64,
    /// The wall clock at the last update: a client frame's `wall_dt`.
    last_wall: f64,
    /// Raw input resolution: held controls, pointer, modifiers, the action
    /// table and the held bindings.
    controls: Controls,
    gui_router: GuiRouter,
    /// GUI-document runtime driver for the window's screens (every screen
    /// is document-backed).
    ui: ui_runtime::AppUi,
    /// The HUD document's own runtime: the scene layer's, solved beside
    /// whatever screen the window shows (see `frame_ui`).
    hud_ui: ui_runtime::AppUi,
    /// Search/selection and filtered rows for player crafting. Presentation
    /// state only; the stable selected key is sent on CRAFT.
    crafting_browser: crafting_browser::CraftingBrowser,
    /// Reused draw list for the window's UI layer.
    composed_doc: petramond_ui::DrawList,
    composed_doc_images: Vec<petramond::gui::DocImageSource>,
    /// Reused draw list for the scene's UI layer (the HUD).
    composed_hud: petramond_ui::DrawList,
    composed_hud_images: Vec<petramond::gui::DocImageSource>,
    /// Open client-WASM physical-pixel canvas, separate from GUI documents;
    /// dropped whenever the canvas screen is left. A pack launched on the
    /// shell opens one with no session, so it lives beside the screen.
    client_canvas: Option<client_mod_ui::ClientCanvasState>,
    /// What the open client document last told its owner about hover and
    /// list ranges (sent on change only).
    client_doc_watch: client_doc_events::DocWatch,
    /// The client UI a mod's `ClientPauseOpen` paused over: Resume returns
    /// to it instead of to gameplay.
    pause_return: Option<(AppScreen, Option<client_mod_ui::ClientCanvasState>)>,
    /// The size the world renders at for a frame-size claim or a viewport
    /// node.
    set_frame_size: Option<(u32, u32)>,
    /// A pack started from its title-screen launch entry, running on the
    /// shell (or lent to the presentation it opened).
    launched: Option<launched::LaunchedShell>,
    /// Reused renderer handoff for always-on client overlays plus the canvas.
    client_overlays: petramond_render::ClientOverlayLayer,
    /// Reused renderer handoff for every client mod's world marks.
    world_marks: petramond_render::WorldMarks,
    /// The current screen. Changed only through the screen funnel
    /// (`screen_flow`), which applies its cursor policy and transient resets.
    screen: AppScreen,
    /// Screens pushed under the current one (the Options flow over the title
    /// or the pause menu); Back pops to the top of this.
    screens_under: Vec<AppScreen>,
    /// The Options flow: persistent settings (`client.json`), slider
    /// previews, the armed control remap, the renderer refresh flag.
    options: options_state::OptionsState,
    /// Presentation time of the last [`render`](Self::render), so the held-item animation
    /// advances by draw time even when the platform coalesces or skips a redraw.
    last_render: f64,
    /// Returns the allocator's free pages to the OS once terrain settles (see
    /// [`heap_reclaim`]).
    heap_reclaim: heap_reclaim::IdleHeapReclaim,
    /// The title flow's state: world list and selection, the open page's
    /// session, the connect and account sessions, the last disconnect reason.
    shell: shell_state::ShellState,
    /// The view the LAST rendered frame actually presented, published to the
    /// client-mod frame hook as `ClientViewState`. Written where the view is
    /// resolved (`render`) and read where the hook is driven, so the snapshot a
    /// mod reads is the frame that presented, not a second derivation of it.
    presented_view: mod_api::ClientViewStateData,
    /// The last session's section cache, harvested when it ended
    /// ([`Session::end`]): the next remote join claims it in its Join
    /// manifest so a reconnect re-promotes cached terrain instead of
    /// re-streaming it. Deliberately BETWEEN sessions, so it lives here.
    retained_section_cache: Option<crate::game::section_cache::SectionCache>,
    quit_requested: bool,
    /// A restart asked to relaunch into this start route once the loop ends.
    relaunch: Option<String>,
    /// What the startup apply of pending content changes did.
    content_report: petramond::content::ApplyReport,
    /// Where this launch was asked to land (`PETRAMOND_START`), until taken.
    start_route: Option<content::StartRoute>,
    /// The content library: listing, downloads, pending changes, and the
    /// browser's own state while it is open.
    content: content::ContentSession,
    renderer_world_clear_pending: bool,
    /// The presented world jumped in time (an apply restated it): the renderer
    /// drops what presents its moment and keeps its terrain.
    renderer_moment_clear_pending: bool,
    /// Frames, time and sound for client mods: the presentation clock, and
    /// the captures, taps and media files the runtime's desk holds.
    media: media::MediaHost,
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
        // The shell camera's field of view is the one nothing has claimed yet:
        // the first value `presented_view` reports, before a frame has drawn.
        let base_fov_y = cam.fov_y;
        let mut app = Self {
            session: None,
            shell_camera: cam,
            render_dist,
            sound,
            last: now_seconds(),
            last_wall: now_seconds(),
            controls: Controls::new(),
            gui_router: GuiRouter::default(),
            ui: ui_runtime::AppUi::new(),
            hud_ui: ui_runtime::AppUi::new(),
            crafting_browser: Default::default(),
            composed_doc: petramond_ui::DrawList::default(),
            composed_doc_images: Vec::new(),
            composed_hud: petramond_ui::DrawList::default(),
            composed_hud_images: Vec::new(),
            client_canvas: None,
            client_doc_watch: Default::default(),
            pause_return: None,
            set_frame_size: None,
            launched: None,
            client_overlays: Default::default(),
            world_marks: Default::default(),
            screen: AppScreen::Title,
            screens_under: Vec::new(),
            options: options_state::OptionsState::new(settings),
            last_render: now_seconds(),
            heap_reclaim: Default::default(),
            shell: Default::default(),
            presented_view: mod_api::ClientViewStateData {
                third_person: false,
                hud_visible: true,
                hands_visible: true,
                crosshair_visible: false,
                camera_claimed: false,
                fov_y: base_fov_y,
                pos: [0.0; 3],
                yaw: 0.0,
                pitch: 0.0,
                roll: 0.0,
                frame_size: [0, 0],
                subject: None,
                anchor_missing: false,
                world_settled: false,
            },
            retained_section_cache: None,
            quit_requested: false,
            relaunch: None,
            content_report: Default::default(),
            start_route: content::StartRoute::take_from_env(),
            content: Default::default(),
            renderer_world_clear_pending: true,
            renderer_moment_clear_pending: false,
            media: Default::default(),
        };
        app.set_screen(AppScreen::Title);
        app.shell.refresh_worlds();
        app
    }

    /// Flush the world to disk on quit: a save request to the server thread.
    /// Dropping the `App` (→ `Game` → `ServerHandle`) then shuts the server
    /// down, which saves again and joins — the request here just bounds the
    /// window if teardown is interrupted.
    pub fn save_on_exit(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.save_all();
        }
    }

    // Known polish gap: no Text (I-beam) cursor over document text inputs yet;
    // every visible-cursor screen uses the default arrow.
    #[inline]
    pub fn cursor_policy(&self) -> CursorPolicy {
        CursorPolicy::for_screen(self.screen)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        // A frame size of its own keeps its shape; the window's comes back
        // when it ends.
        if self.set_frame_size.is_none() {
            self.apply_aspect(width, height);
        }
    }

    fn apply_aspect(&mut self, width: u32, height: u32) {
        let aspect = width as f32 / height.max(1) as f32;
        self.shell_camera.aspect = aspect;
        if let Some(session) = self.session.as_mut() {
            session.game.set_aspect(aspect);
        }
    }

    /// Apply a shared control event. Returns false only when the app did not
    /// consume the control, e.g. Escape with no screen open on native.
    pub fn handle_control(&mut self, control: Control, down: bool) -> bool {
        let Some(event) = self.controls.input.set_control(control, down) else {
            return true;
        };
        // A presentation is watched, not played: only moving, looking, the
        // perspective and pausing act on it (mod key actions dispatch
        // elsewhere, as ever). A release always lands so nothing sticks.
        let watching = self
            .session
            .as_ref()
            .is_some_and(|session| session.game.in_presentation());
        if watching && !acts_in_presentation(&event) {
            return true;
        }

        match event {
            ControlEvent::OpenChat { command } => {
                if self.screen == AppScreen::Game && self.session.is_some() {
                    self.set_screen(AppScreen::Chat);
                    let now = self.now();
                    if let Some(session) = self.session.as_mut().filter(|_| command) {
                        session.chat.insert_text("/", now);
                    }
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
                if self.session.is_some()
                    && (self.screen.gameplay_enabled() || self.screen.ui_open())
                {
                    self.toggle_inventory();
                }
                true
            }
            ControlEvent::ToggleCreative
            | ControlEvent::UndoEdit
            | ControlEvent::RedoEdit
            | ControlEvent::JumpPressed => {
                let now = self.now();
                if self.screen.gameplay_enabled() {
                    if let Some(Session { game, .. }) = self.session.as_mut() {
                        match event {
                            ControlEvent::ToggleCreative => game.toggle_creative_mode(),
                            ControlEvent::UndoEdit => game.undo_edit(),
                            ControlEvent::RedoEdit => game.redo_edit(),
                            _ => game.jump_pressed(now),
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
                        .session
                        .as_mut()
                        .is_some_and(|session| session.game.adjust_tool(steps));
                if !taken {
                    self.controls
                        .input
                        .step_hotbar(self.hotbar_step_for_adjust(steps));
                }
                true
            }
            ControlEvent::TogglePlayerMode => {
                if self.screen.gameplay_enabled() {
                    if let Some(Session { game, .. }) = self.session.as_mut() {
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
                    if let Some(Session { game, .. }) = self.session.as_mut() {
                        game.set_active_hotbar(slot);
                    }
                }
                true
            }
            ControlEvent::DropItem => {
                if self.screen.ui_open() {
                    let (x, y) = self.controls.pointer.cursor();
                    if let (Some(slot), Some(Session { game, .. })) =
                        (self.ui.menu_slot_at(x, y), self.session.as_mut())
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
                    if let Some(Session { game, .. }) = self.session.as_mut() {
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
                    if let (Some(slot), Some(Session { game, .. })) =
                        (self.ui.menu_slot_at(x, y), self.session.as_mut())
                    {
                        game.menu_swap_off_hand(slot);
                    }
                } else if self.screen.gameplay_enabled() {
                    if let Some(Session { game, .. }) = self.session.as_mut() {
                        game.swap_off_hand();
                    }
                }
                true
            }
            ControlEvent::RotateHeldBlock => {
                if self.screen.gameplay_enabled() {
                    if let Some(Session { game, .. }) = self.session.as_mut() {
                        if !game.rotate_schematic_preview() {
                            game.toggle_held_block_rotation();
                        }
                    }
                }
                true
            }
            ControlEvent::TogglePerspective => {
                if self.screen.gameplay_enabled() {
                    if let Some(Session { game, .. }) = self.session.as_mut() {
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
            .options
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
        self.ui
            .push_input(petramond_ui::InputEvent::Modifiers(petramond_ui::Mods {
                ctrl: modifiers.ctrl,
                shift: modifiers.shift,
                alt: modifiers.alt,
            }));
        let mut out = Vec::new();
        self.controls
            .binding_engine
            .on_modifiers_changed(modifiers, &mut out);
        self.dispatch_actions(out);
    }

    /// Which GUI document backs the current screen, if any: the screen
    /// table's document, when it is loaded. Document-backed screens draw +
    /// route input through the petramond-ui runtime; a screen with no loaded
    /// document draws (and routes) nothing.
    pub fn doc_ui_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        self.screen
            .spec()
            .doc
            .filter(|&kind| ui_runtime::AppUi::doc_backed(kind))
    }

    /// The subset of [`doc_ui_kind`](Self::doc_ui_kind) where the whole frame
    /// belongs to the shell (no game simulation behind it). Game menus (mod
    /// GUIs, containers) return `None` here — they drive their document UI
    /// AND tick the game.
    pub fn doc_shell_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        self.doc_kind_for(screen::ScreenRole::Shell)
    }

    /// The subset of [`doc_ui_kind`](Self::doc_ui_kind) for gameplay OVERLAY
    /// screens (sleep / death): the document owns input like a shell screen —
    /// its events dispatch to a controller, not to slot routing — but the
    /// simulation keeps ticking underneath (the sleep timer and respawn are
    /// tick-owned).
    pub fn doc_overlay_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        self.doc_kind_for(screen::ScreenRole::Overlay)
    }

    fn doc_kind_for(
        &self,
        role: screen::ScreenRole,
    ) -> Option<petramond_world::gui_state::GuiKind> {
        (self.screen.role() == role)
            .then(|| self.doc_ui_kind())
            .flatten()
    }

    /// Whether the hotbar HUD draws from its GUI document this frame
    /// (HUD screens only, as the scene sees it; presentation-only, input
    /// stays with the game).
    pub fn doc_hud_active(&self) -> bool {
        self.scene_screen().spec().hud
            && self.session.is_some()
            && ui_runtime::AppUi::doc_backed(petramond_world::gui_state::GuiKind::Hotbar)
    }
}

fn now_seconds() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// The controls that act while a presentation presents.
fn acts_in_presentation(event: &input::ControlEvent) -> bool {
    use input::ControlEvent as E;
    match event {
        E::CloseScreen | E::TogglePerspective | E::JumpPressed => true,
        E::Attack { down } | E::Interact { down } => !down,
        E::ToggleInventory
        | E::OpenChat { .. }
        | E::TogglePlayerMode
        | E::ToggleCreative
        | E::UndoEdit
        | E::RedoEdit
        | E::AdjustTool(_)
        | E::SelectHotbar(_)
        | E::DropItem
        | E::SwapOffHand
        | E::RotateHeldBlock => false,
    }
}

#[cfg(test)]
mod tests;
