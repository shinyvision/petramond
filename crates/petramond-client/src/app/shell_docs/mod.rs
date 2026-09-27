//! Controllers for document-backed shell screens.
//!
//! Each screen is a GUI document (`assets/ui/documents/<name>.gui.json`) plus
//! one controller module here: `populate` writes the screen's dynamic values
//! into the [`petramond_ui::UiState`] the document binds, and `handle` maps the
//! frame's resolved [`petramond_ui::UiEvent`]s to app actions (screen
//! transitions, world I/O). A screen routes through here exactly when
//! [`App::doc_ui_kind`] maps it and its document loads.
//!
//! Controllers never see the `App`: each hook gets a [`ScreenCtx`] holding
//! only the shell state, the options, the screen's document runtime and a
//! read-only view of the session, and queues [`ShellCommand`]s for anything
//! app-level (transitions, sessions, applying options), which the App runs
//! after the frame. Runs from [`App::update`], never from render.

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

/// The flat dim shell menus draw over a live world.
const MENU_DIM: [f32; 4] = [0.0, 0.0, 0.0, 0.6];

/// One document-backed shell screen, named once: how to bind its state, how
/// to dispatch its events, and what dims the world behind it. Every hook
/// sees only the [`ScreenCtx`] — the shell's state, the options, the
/// screen's document and a view of the session — never the App.
struct ShellController {
    /// Bespoke per-frame prep before binding (worker polls, extra images).
    /// Returning false means the prep requested a screen switch — skip the
    /// frame rather than draw the stale document.
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

/// Options screens over a paused/running game dim like the pause menu; from
/// the title flow the document's own backdrop shows.
fn options_dim(ctx: &ScreenCtx) -> Option<[f32; 4]> {
    ctx.session.in_game.then_some(MENU_DIM)
}

/// Shared prepare for the screens whose Mods tab shows per-pack icons.
fn pack_icon_prepare(ctx: &mut ScreenCtx) -> bool {
    ctx.ui
        .set_dynamic_images(crate::app::content::pack_icons().to_vec());
    true
}

/// Shared options-family chrome: the title flow shows the document's
/// screenshot backdrop; over a live game the host dim does the work instead.
fn populate_options_chrome(ctx: &ScreenCtx, state: &mut UiState) {
    state.set("show_backdrop", UiValue::Bool(!ctx.session.in_game));
}

/// Shared Back handling for the options CATEGORY screens (Sound / Controls /
/// Graphics): Back returns to the Options root through the same path ESC
/// takes. Returns true when the event was consumed.
fn options_category_back(ctx: &mut ScreenCtx, ev: &UiEvent) -> bool {
    if matches!(ev, UiEvent::Click { id, .. } if id.as_str() == "back") {
        ctx.request(ShellCommand::Back);
        return true;
    }
    false
}

/// The Connect screen's prep: consume the connect worker's outcomes BEFORE
/// binding. A join or a mod refusal switches screens — skip the rest of this
/// frame rather than draw the stale connect UI.
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
        // The tick-driven darkening fade behind the sleep overlay.
        GuiKind::Sleep => C::screen(sleep::populate, sleep::handle).with_dim(|ctx| {
            let progress = ctx.session.sleep_progress.unwrap_or(1.0);
            Some([0.0, 0.0, 0.0, 0.25 + 0.75 * progress])
        }),
        GuiKind::Death => {
            C::screen(death::populate, death::handle).with_dim(|_| Some([0.35, 0.02, 0.02, 0.40]))
        }
        // Unrouted kinds still run an inert frame, as before.
        _ => C::screen(|_, _| {}, |_, _| {}),
    }
}

/// Test support: the controls-list row index of an action id (category
/// headers count), resolved through the controller's own row builder.
#[cfg(test)]
pub(in crate::app) fn controls_action_row_index(
    table: &petramond_input::controls::ActionTable,
    action_id: &str,
) -> Option<usize> {
    options_controls::row_entries(table)
        .iter()
        .position(|e| matches!(e, options_controls::RowEntry::Action(id) if id == action_id))
}

/// The widget id a GAME-MENU event activates — the one lane that reaches a
/// mod through [`crate::game::Game::menu_click`].
///
/// A toggle rides it beside a button click because a machine's switch IS a
/// widget the mod acts on, and the mod owns whether it ends up on: the node's
/// own latch is presentation, the bound value is the truth. BOTH stay
/// primary-only, like the legacy dispatch — a right-click on a machine's lever
/// is not a pull, and this lane is the only thing standing between a stray
/// secondary press and a mod acting on it.
///
/// Pulled out of `drive_doc_menu` because everything downstream of this hop is
/// covered (`game/tests/menu.rs`) and the hop itself needs a live App with a
/// mod document to reach any other way.
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

/// A press on a control by the SECONDARY button, which the shell drops before
/// it reaches a screen's controller.
///
/// Buttons and checkboxes are primary-only, the same rule the game-menu lane
/// applies ([`menu_widget_activation`]): secondary is the cursor-stack gesture
/// wherever it means anything, and never a press on a control. It is filtered
/// in one place rather than per screen because "which button pressed me" is
/// not a question each Back button and each options checkbox should answer
/// separately — and every screen that forgot to ask flipped under a
/// right-click.
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
    /// Drive one frame of the document UI for `kind`: populate bound state,
    /// run the runtime over the queued input, then dispatch the resolved
    /// events to the screen's controller.
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

    /// Carry out one app-level request a shell screen queued.
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

    /// Drive one frame of a document-backed GAME MENU (mod GUIs and
    /// containers): bound values come from the tick-owned GUI state map and
    /// the container views; slot clicks/drags/drops and widget clicks latch
    /// to the tick as
    /// [`petramond_world::gui_state::MenuSlot`] clicks — the same deterministic path the
    /// legacy hit-test used. Off-panel presses throw the cursor stack.
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
            // Every gauge — an engine machine's or a pack's — arrives as an
            // ordinary named GUI-state value; nothing here knows a furnace.
            if let Some(map) = session.game.menu_read_model().gui_state {
                for (key, value) in map.iter() {
                    let v = crate::app::gui_value::from_world(value);
                    state.set(key.clone(), v);
                }
            }
        }
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
        for ev in self.ui.take_events() {
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
