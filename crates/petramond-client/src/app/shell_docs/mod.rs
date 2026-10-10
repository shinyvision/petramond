//! Controllers for document-backed shell screens.
//!
//! Each screen is a GUI document (`assets/ui/documents/<name>.gui.json`) plus a
//! controller module here. `populate` fills the [`petramond_ui::UiState`] the
//! document binds. `handle` maps resolved [`petramond_ui::UiEvent`]s to app
//! actions (transitions, world I/O). A screen routes here only when
//! [`App::doc_ui_kind`] maps it and its document loads.
//!
//! Controllers never see the `App`. Each hook gets a [`ScreenCtx`] with the shell
//! state, options, the screen's document runtime and a read-only session view.
//! Anything app-level is queued as a [`ShellCommand`] for the App to run after
//! the frame. Runs from [`App::update`], never from render.

mod account;
pub(in crate::app) mod account_sign_in;
pub(in crate::app) mod connect_server;
mod connection_lost;
mod content;
pub(in crate::app) use content::rows::locals_from_discovery as content_locals;
pub(in crate::app) use content::ContentView;
mod context;
mod create_world;
mod death;
mod delete_world;
mod mods_missing;
pub(crate) use mods_missing::MissingWorld;
mod mods_tab;
mod options;
mod options_controls;
mod options_graphics;
mod options_sound;
mod pause;
mod sleep;
mod title;
mod world_select;
mod world_settings;

pub(in crate::app) use context::{ScreenCtx, SessionFacts, ShellCommand};

use super::session::Session;
use super::App;
use petramond_ui::{UiEvent, UiState, UiValue};
use petramond_world::gui_state::GuiKind;
use petramond_world::sound_registry::Sound;

const MENU_DIM: [f32; 4] = [0.0, 0.0, 0.0, 0.6];

struct ShellController {
    prepare: Option<fn(&mut ScreenCtx) -> bool>,
    populate: fn(&ScreenCtx, &mut UiState),
    handle: fn(&mut ScreenCtx, UiEvent),
    dim: fn(&ScreenCtx) -> Option<[f32; 4]>,
}

impl ShellController {
    fn screen(populate: fn(&ScreenCtx, &mut UiState), handle: fn(&mut ScreenCtx, UiEvent)) -> Self {
        ShellController {
            prepare: None,
            populate,
            handle,
            dim: |_| None,
        }
    }

    fn with_prepare(mut self, prepare: fn(&mut ScreenCtx) -> bool) -> Self {
        self.prepare = Some(prepare);
        self
    }

    fn with_dim(mut self, dim: fn(&ScreenCtx) -> Option<[f32; 4]>) -> Self {
        self.dim = dim;
        self
    }
}

fn options_dim(ctx: &ScreenCtx) -> Option<[f32; 4]> {
    ctx.session.in_game.then_some(MENU_DIM)
}

fn pack_icon_prepare(ctx: &mut ScreenCtx) -> bool {
    ctx.ui
        .set_dynamic_images(crate::app::content::pack_icons().to_vec());
    true
}

fn populate_options_chrome(ctx: &ScreenCtx, state: &mut UiState) {
    state.set("show_backdrop", UiValue::Bool(!ctx.session.in_game));
}

fn options_category_back(ctx: &mut ScreenCtx, ev: &UiEvent) -> bool {
    if matches!(ev, UiEvent::Click { id, .. } if id.as_str() == "back") {
        ctx.request(ShellCommand::Back);
        return true;
    }
    false
}

fn connect_prepare(ctx: &mut ScreenCtx) -> bool {
    match ctx.shell.connect.poll() {
        Some(event) => {
            ctx.request(crate::app::connect::connect_event_command(event));
            false
        }
        None => true,
    }
}

fn controller_for(kind: GuiKind) -> ShellController {
    use ShellController as C;
    match kind {
        GuiKind::Demo => C::screen(
            |_, state| super::ui_runtime::demo::populate(state),
            |ctx, ev| super::ui_runtime::demo::apply_one(ctx.ui.state_mut(), &ev),
        ),
        GuiKind::Title => C::screen(title::populate, title::handle).with_prepare(title::prepare),
        GuiKind::WorldSelect => C::screen(world_select::populate, world_select::handle),
        GuiKind::WorldSettings => C::screen(world_settings::populate, world_settings::handle)
            .with_prepare(world_settings::prepare),
        GuiKind::CreateWorld => {
            C::screen(create_world::populate, create_world::handle).with_prepare(pack_icon_prepare)
        }
        GuiKind::DeleteWorld => C::screen(delete_world::populate, delete_world::handle),
        GuiKind::ConnectServer => C::screen(connect_server::populate, connect_server::handle)
            .with_prepare(connect_prepare),
        GuiKind::Account => {
            C::screen(account::populate, account::handle).with_prepare(account::prepare)
        }
        GuiKind::AccountSignIn => C::screen(account_sign_in::populate, account_sign_in::handle)
            .with_prepare(account_sign_in::prepare),
        GuiKind::Content => {
            C::screen(content::populate, content::handle).with_prepare(content::prepare)
        }
        GuiKind::ModsMissing => C::screen(mods_missing::populate, mods_missing::handle),
        GuiKind::ConnectionLost => C::screen(connection_lost::populate, connection_lost::handle),
        GuiKind::Options => C::screen(options::populate, options::handle).with_dim(options_dim),
        GuiKind::OptionsSound => {
            C::screen(options_sound::populate, options_sound::handle).with_dim(options_dim)
        }
        GuiKind::OptionsControls => {
            C::screen(options_controls::populate, options_controls::handle).with_dim(options_dim)
        }
        GuiKind::OptionsGraphics => {
            C::screen(options_graphics::populate, options_graphics::handle).with_dim(options_dim)
        }
        GuiKind::Pause => C::screen(pause::populate, pause::handle).with_dim(|_| Some(MENU_DIM)),
        GuiKind::Sleep => C::screen(sleep::populate, sleep::handle).with_dim(|ctx| {
            let progress = ctx.session.sleep_progress.unwrap_or(1.0);
            Some([0.0, 0.0, 0.0, 0.25 + 0.75 * progress])
        }),
        GuiKind::Death => {
            C::screen(death::populate, death::handle).with_dim(|_| Some([0.35, 0.02, 0.02, 0.40]))
        }
        _ => C::screen(|_, _| {}, |_, _| {}),
    }
}

#[cfg(test)]
pub(in crate::app) fn controls_action_row_index(
    table: &petramond_input::controls::ActionTable,
    action_id: &str,
) -> Option<usize> {
    options_controls::row_entries(table)
        .iter()
        .position(|e| matches!(e, options_controls::RowEntry::Action(id) if id == action_id))
}

pub(in crate::app) fn menu_widget_activation(ev: &petramond_ui::UiEvent) -> Option<&str> {
    match ev {
        petramond_ui::UiEvent::Click {
            id,
            button: petramond_ui::PointerButton::Primary,
            ..
        }
        | petramond_ui::UiEvent::Toggle {
            id,
            button: petramond_ui::PointerButton::Primary,
            ..
        } => Some(id),
        _ => None,
    }
}

pub(super) fn is_secondary_activation(ev: &petramond_ui::UiEvent) -> bool {
    use petramond_ui::PointerButton::Secondary;
    matches!(
        ev,
        petramond_ui::UiEvent::Click {
            button: Secondary,
            ..
        } | petramond_ui::UiEvent::Toggle {
            button: Secondary,
            ..
        }
    )
}

fn is_widget_activation(ev: &petramond_ui::UiEvent) -> bool {
    matches!(
        ev,
        petramond_ui::UiEvent::Click { .. }
            | petramond_ui::UiEvent::Toggle { .. }
            | petramond_ui::UiEvent::TabSelect { .. }
    )
}

impl App {
    pub(super) fn drive_doc_ui(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        self.ui.ensure_active(kind);
        let ctl = controller_for(kind);
        let now_presented = self.now();
        let live = self.session.as_ref();
        let session = SessionFacts {
            in_game: live.is_some(),
            is_remote: live.is_some_and(|s| s.game.is_remote()),
            presenting: live.is_some_and(|s| s.game.in_presentation()),
            lan_port: live.and_then(|s| s.lan_port),
            lan_error: live.and_then(|s| s.lan_error.as_deref()),
            sleep_counts: live
                .map(|s| s.game.sleeping_player_counts())
                .unwrap_or((0, 1)),
            sleep_progress: live.and_then(|s| s.game.sleep_progress01()),
        };
        let mut ctx = ScreenCtx::new(
            &mut self.shell,
            &mut self.options,
            (&mut self.content, &self.content_report),
            &self.controls.action_table,
            &mut self.ui,
            session,
            now_presented,
        );
        let proceed = ctl.prepare.is_none_or(|prepare| prepare(&mut ctx));
        if proceed {
            let mut state = std::mem::take(ctx.ui.state_mut());
            (ctl.populate)(&ctx, &mut state);
            *ctx.ui.state_mut() = state;
            let dim = (ctl.dim)(&ctx);
            ctx.ui.frame(kind, screen, now, dim);
            for ev in ctx.ui.take_events() {
                if is_secondary_activation(&ev) {
                    continue;
                }
                if is_widget_activation(&ev) {
                    self.sound.play_interface(Sound::UiClick);
                }
                (ctl.handle)(&mut ctx, ev);
            }
        }
        for command in ctx.into_commands() {
            self.run_shell_command(command);
        }
    }

    pub(super) fn run_shell_command(&mut self, command: ShellCommand) {
        match command {
            ShellCommand::Goto(screen) => self.set_screen(screen),
            ShellCommand::Push(screen) => self.push_screen(screen),
            ShellCommand::Back => self.go_back(),
            ShellCommand::Exit(kind) => self.request_exit(kind),
            ShellCommand::ExitNow(kind) => self.exit_now(kind),
            ShellCommand::PlaySelectedWorld => self.play_selected_world(),
            ShellCommand::StartGame { dir_name, seed } => self.start_game(&dir_name, seed),
            ShellCommand::OpenConnectServer => self.open_connect_server(),
            ShellCommand::BeginConnect => self.begin_connect(),
            ShellCommand::LeaveModsMissing => self.leave_mods_missing(),
            ShellCommand::OpenContent { back, filter } => self.open_content(back, filter),
            ShellCommand::CloseContent => self.close_content(),
            ShellCommand::ApplyContent => self.request_content_apply(),
            ShellCommand::OpenAccount(status) => self.open_account(status),
            ShellCommand::LeaveAccount => self.leave_account(),
            ShellCommand::OpenAccountSignIn => self.open_account_sign_in(),
            ShellCommand::LeaveAccountSignIn => self.leave_account_sign_in(),
            ShellCommand::SubmitAccountSignIn => self.submit_account_sign_in(),
            ShellCommand::AccountSignOut => self.account_sign_out(),
            ShellCommand::LaunchPack { pack_id, screen } => self.launch_pack(&pack_id, screen),
            ShellCommand::EndPresentation => self.end_presentation(),
            ShellCommand::AdoptRemote(join, handle) => self.start_remote_game(*join, handle),
            ShellCommand::ResumeGame => self.resume_game(),
            ShellCommand::OpenLan => self.open_lan(),
            ShellCommand::DisconnectToTitle => self.disconnect_to_title(),
            ShellCommand::SaveAndQuitToTitle => self.save_and_quit_to_title(),
            ShellCommand::CancelSleep => self.cancel_sleep(),
            ShellCommand::Respawn => {
                if let Some(session) = self.session.as_mut() {
                    session.game.request_respawn();
                }
            }
            ShellCommand::ApplyVolumes => self.apply_volumes(),
            ShellCommand::ApplyParticles => self.apply_particles(),
            ShellCommand::ApplyViewDistance(chunks) => self.apply_view_distance(chunks),
        }
    }

    pub(super) fn drive_doc_menu(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        if kind == GuiKind::Creative {
            self.drive_creative_menu(screen, now);
            return;
        }
        self.ui.ensure_active(kind);
        let crafting_station = petramond_world::crafting::CraftingStation::of_kind(kind);
        if let (Some(station), Some(session)) = (crafting_station, self.session.as_ref()) {
            let hovered = self
                .ui
                .hover_item(crate::app::crafting_browser::RECIPE_LIST_ID);
            let mut state = std::mem::take(self.ui.state_mut());
            self.crafting_browser
                .populate(&session.game, station, hovered, &mut state);
            *self.ui.state_mut() = state;
        }
        if let Some(session) = self.session.as_ref() {
            let state = self.ui.state_mut();
            if let Some(map) = session.game.menu_read_model().gui_state {
                for (key, value) in map.iter() {
                    let v = crate::app::gui_value::from_world(value);
                    state.set(key.clone(), v);
                }
            }
        }
        self.overlay_co_driven_view();
        if let Some(Session { game, .. }) = self.session.as_ref() {
            let hover_slot = self.ui.out().hover_slot.clone();
            let images =
                crate::app::item_tooltip::populate(game, hover_slot.as_ref(), self.ui.state_mut());
            self.ui.set_extra_images(&images);
            crate::app::item_tooltip::populate_slot_tip(
                game,
                hover_slot.as_ref(),
                self.ui.state_mut(),
            );
        }
        self.ui.frame(kind, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        let modifier_shift = self.controls.modifiers.shift;
        let events = self.ui.take_events();
        if let Some(kind_key) = self.co_driven_menu() {
            self.forward_client_doc_events(kind, kind_key, &events);
        }
        for ev in events {
            if is_widget_activation(&ev) && !is_secondary_activation(&ev) {
                self.sound.play_interface(Sound::UiClick);
            }
            let handled_crafting = if crafting_station.is_some() {
                self.session.as_mut().is_some_and(|session| {
                    self.crafting_browser
                        .handle(&mut session.game, &ev, modifier_shift)
                })
            } else {
                false
            };
            if handled_crafting {
                continue;
            }
            if let Some(id) = menu_widget_activation(&ev) {
                if let Some(session) = self.session.as_mut() {
                    session.game.menu_click(
                        petramond_world::gui_state::MenuSlot::Widget(
                            petramond_world::gui_state::intern_str(id),
                        ),
                        petramond_world::gui_state::PointerButton::Primary,
                        modifier_shift,
                        false,
                    );
                }
                continue;
            }
            self.handle_inventory_event(kind, ev, now);
        }
    }
}
