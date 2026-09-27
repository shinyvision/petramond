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
    session: Option<Session>,
    shell_camera: Camera,
    render_dist: i32,
    sound: client_audio::ClientAudio,
    last: f64,
    last_wall: f64,
    controls: Controls,
    gui_router: GuiRouter,
    ui: ui_runtime::AppUi,
    hud_ui: ui_runtime::AppUi,
    crafting_browser: crafting_browser::CraftingBrowser,
    composed_doc: petramond_ui::DrawList,
    composed_doc_images: Vec<petramond::gui::DocImageSource>,
    composed_hud: petramond_ui::DrawList,
    composed_hud_images: Vec<petramond::gui::DocImageSource>,
    client_canvas: Option<client_mod_ui::ClientCanvasState>,
    client_doc_watch: client_doc_events::DocWatch,
    pause_return: Option<(AppScreen, Option<client_mod_ui::ClientCanvasState>)>,
    set_frame_size: Option<(u32, u32)>,
    launched: Option<launched::LaunchedShell>,
    client_overlays: petramond_render::ClientOverlayLayer,
    world_marks: petramond_render::WorldMarks,
    screen: AppScreen,
    screens_under: Vec<AppScreen>,
    options: options_state::OptionsState,
    last_render: f64,
    heap_reclaim: heap_reclaim::IdleHeapReclaim,
    shell: shell_state::ShellState,
    presented_view: mod_api::ClientViewStateData,
    retained_section_cache: Option<crate::game::section_cache::SectionCache>,
    quit_requested: bool,
    relaunch: Option<String>,
    content_report: petramond::content::ApplyReport,
    start_route: Option<content::StartRoute>,
    content: content::ContentSession,
    content_apply_requested: bool,
    renderer_world_clear_pending: bool,
    renderer_moment_clear_pending: bool,
    media: media::MediaHost,
}

impl App {
    pub fn new(cam: Camera, render_dist: i32) -> Self {
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
            content_apply_requested: false,
            renderer_world_clear_pending: true,
            renderer_moment_clear_pending: false,
            media: Default::default(),
        };
        app.set_screen(AppScreen::Title);
        app.shell.refresh_worlds();
        app
    }

    pub fn save_on_exit(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.game.save_all();
        }
    }

    #[inline]
    pub fn cursor_policy(&self) -> CursorPolicy {
        CursorPolicy::for_screen(self.screen)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
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

    pub fn handle_control(&mut self, control: Control, down: bool) -> bool {
        let Some(event) = self.controls.input.set_control(control, down) else {
            return true;
        };
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
            _ if self.screen == AppScreen::Chat => match event {
                ControlEvent::CloseScreen => self.close_screen(),
                _ => true,
            },
            ControlEvent::ToggleInventory => {
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
                        game.menu_drop(slot, self.controls.modifiers.ctrl);
                    }
                } else if self.screen.gameplay_enabled() {
                    let whole_stack = self.controls.input.sprint_held();
                    if let Some(Session { game, .. }) = self.session.as_mut() {
                        game.drop_selected_item(whole_stack);
                    }
                }
                true
            }
            ControlEvent::SwapOffHand => {
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

    pub fn doc_ui_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        self.screen
            .spec()
            .doc
            .filter(|&kind| ui_runtime::AppUi::doc_backed(kind))
    }

    pub fn doc_shell_kind(&self) -> Option<petramond_world::gui_state::GuiKind> {
        self.doc_kind_for(screen::ScreenRole::Shell)
    }

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
