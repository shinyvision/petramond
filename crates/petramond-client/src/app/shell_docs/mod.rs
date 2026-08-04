//! Controllers for document-backed shell screens.
//!
//! Each screen is a GUI document (`assets/ui/documents/<name>.gui.json`) plus
//! one controller module here: `populate` writes the screen's dynamic values
//! into the [`petramond_ui::UiState`] the document binds, and `handle` maps the
//! frame's resolved [`petramond_ui::UiEvent`]s to app actions (screen
//! transitions, world I/O). A screen routes through here exactly when
//! [`App::doc_ui_kind`] maps it and its document loads.
//!
//! Runs from [`App::update`], never from render — controllers mutate
//! app-shell state, and presentation only hands the already-built draw list
//! to the renderer.

mod connect_server;
mod connection_lost;
mod create_world;
mod death;
mod delete_world;
mod mods_missing;
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

use super::{App, AppScreen};
use petramond_world::sound_registry::Sound;
use petramond_world::gui_state::GuiKind;
use petramond_ui::{UiEvent, UiState, UiValue};

/// The flat dim shell menus draw over a live world.
const MENU_DIM: [f32; 4] = [0.0, 0.0, 0.0, 0.6];

/// One document-backed shell screen, named once: how to bind its state, how
/// to dispatch its events, and what dims the world behind it.
struct ShellController {
    /// Bespoke per-frame prep before binding (worker polls, extra images).
    /// Returning false means the prep switched screens — skip the frame
    /// rather than draw the stale document.
    prepare: Option<fn(&mut App) -> bool>,
    populate: fn(&App, &mut UiState),
    handle: fn(&mut App, UiEvent),
    dim: fn(&App) -> Option<[f32; 4]>,
}

impl ShellController {
    fn screen(populate: fn(&App, &mut UiState), handle: fn(&mut App, UiEvent)) -> Self {
        ShellController {
            prepare: None,
            populate,
            handle,
            dim: |_| None,
        }
    }

    fn with_prepare(mut self, prepare: fn(&mut App) -> bool) -> Self {
        self.prepare = Some(prepare);
        self
    }

    fn with_dim(mut self, dim: fn(&App) -> Option<[f32; 4]>) -> Self {
        self.dim = dim;
        self
    }
}

/// Options screens over a paused/running game dim like the pause menu; from
/// the title flow the document's own backdrop shows.
fn options_dim(app: &App) -> Option<[f32; 4]> {
    app.game.is_some().then_some(MENU_DIM)
}

/// Shared prepare for the screens whose Mods tab shows per-pack icons.
fn pack_icon_prepare(app: &mut App) -> bool {
    let icons = mods_tab::extra_images();
    app.ui.set_extra_images(&icons);
    true
}

/// Shared options-family chrome: the title flow shows the document's
/// screenshot backdrop; over a live game the host dim does the work instead.
fn populate_options_chrome(app: &App, state: &mut UiState) {
    state.set("show_backdrop", UiValue::Bool(app.game.is_none()));
}

/// Shared Back handling for the options CATEGORY screens (Sound / Controls /
/// Graphics): Back returns to the Options root through the same path ESC
/// takes. Returns true when the event was consumed.
fn options_category_back(app: &mut App, ev: &UiEvent) -> bool {
    if matches!(ev, UiEvent::Click { id, .. } if id.as_str() == "back") {
        app.close_options_category();
        return true;
    }
    false
}

fn controller_for(kind: GuiKind) -> ShellController {
    use ShellController as C;
    match kind {
        GuiKind::Demo => C::screen(
            |_, state| super::ui_runtime::demo::populate(state),
            |app, ev| super::ui_runtime::demo::apply_one(app.ui.state_mut(), &ev),
        ),
        GuiKind::Title => C::screen(title::populate, title::handle),
        GuiKind::WorldSelect => C::screen(world_select::populate, world_select::handle),
        GuiKind::WorldSettings => C::screen(world_settings::populate, world_settings::handle)
            .with_prepare(world_settings::prepare),
        GuiKind::CreateWorld => {
            C::screen(create_world::populate, create_world::handle).with_prepare(pack_icon_prepare)
        }
        GuiKind::DeleteWorld => C::screen(delete_world::populate, delete_world::handle),
        GuiKind::ConnectServer => C::screen(connect_server::populate, connect_server::handle)
            .with_prepare(|app| {
                // Consume the connect worker's outcomes BEFORE binding. A
                // terminal outcome switches screens — skip the rest of this
                // frame rather than draw the stale connect UI.
                app.poll_connect_worker();
                matches!(app.screen, AppScreen::ConnectServer)
            }),
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
        GuiKind::Sleep => C::screen(sleep::populate, sleep::handle).with_dim(|app| {
            let progress = app
                .game
                .as_ref()
                .and_then(|g| g.sleep_progress01())
                .unwrap_or(1.0);
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
    table: &petramond_world::controls::ActionTable,
    action_id: &str,
) -> Option<usize> {
    options_controls::row_entries(table)
        .iter()
        .position(|e| matches!(e, options_controls::RowEntry::Action(id) if id == action_id))
}

/// Split-borrow helper: controllers read `&App` while writing the UI state.
fn with_state(app: &mut App, f: impl FnOnce(&App, &mut UiState)) {
    let mut state = std::mem::take(app.ui.state_mut());
    f(app, &mut state);
    *app.ui.state_mut() = state;
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
fn is_secondary_activation(ev: &petramond_ui::UiEvent) -> bool {
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

fn is_shell_activation(ev: &petramond_ui::UiEvent) -> bool {
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
        if let Some(prepare) = ctl.prepare {
            if !prepare(self) {
                return;
            }
        }
        with_state(self, ctl.populate);
        let dim = (ctl.dim)(self);
        self.ui.frame(kind, screen, now, dim);
        for ev in self.ui.take_events() {
            if is_secondary_activation(&ev) {
                continue;
            }
            if is_shell_activation(&ev) {
                self.audio.play(Sound::UiClick);
            }
            (ctl.handle)(self, ev);
        }
    }

    /// Drive one frame of a document-backed GAME MENU (mod GUIs and
    /// containers): bound values come from the tick-owned GUI state map and
    /// the container views; slot clicks/drags/drops and widget clicks latch
    /// to the tick as
    /// [`petramond_world::gui_state::MenuSlot`] clicks — the same deterministic path the
    /// legacy hit-test used. Off-panel presses throw the cursor stack.
    pub(super) fn drive_doc_menu(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        self.ui.ensure_active(kind);
        let crafting_station = petramond_world::crafting::CraftingStation::of_kind(kind);
        if let (Some(station), Some(game)) = (crafting_station, self.game.as_ref()) {
            let hovered = self
                .ui
                .hover_item(crate::app::crafting_browser::RECIPE_LIST_ID);
            let mut state = std::mem::take(self.ui.state_mut());
            self.crafting_browser
                .populate(game, station, hovered, &mut state);
            *self.ui.state_mut() = state;
        }
        if let Some(game) = self.game.as_ref() {
            let menu = game.menu_read_model();
            let gui_state = menu.gui_state;
            let state = self.ui.state_mut();
            // Every gauge — an engine machine's or a pack's — arrives as an
            // ordinary named GUI-state value; nothing here knows a furnace.
            if let Some(map) = gui_state {
                for (key, value) in map.iter() {
                    let v = match value {
                        petramond_world::gui_state::GuiValue::F32(v) => petramond_ui::UiValue::F32(*v),
                        petramond_world::gui_state::GuiValue::I32(v) => petramond_ui::UiValue::I32(*v),
                        petramond_world::gui_state::GuiValue::Str(s) => petramond_ui::UiValue::Str(s.clone()),
                    };
                    state.set(key.clone(), v);
                }
            }
        }
        if let Some(game) = self.game.as_ref() {
            let hover_slot = self.ui.out().hover_slot.clone();
            crate::app::item_tooltip::populate(game, hover_slot.as_ref(), self.ui.state_mut());
        }
        self.ui.frame(kind, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        let modifier_shift = self.modifiers.shift;
        let to_button = |b: petramond_ui::PointerButton| match b {
            petramond_ui::PointerButton::Primary => petramond_world::gui_state::PointerButton::Primary,
            petramond_ui::PointerButton::Secondary => petramond_world::gui_state::PointerButton::Secondary,
        };
        for ev in self.ui.take_events() {
            let handled_crafting = if crafting_station.is_some() {
                self.game
                    .as_mut()
                    .is_some_and(|game| self.crafting_browser.handle(game, &ev, modifier_shift))
            } else {
                false
            };
            if handled_crafting {
                continue;
            }
            if let Some(id) = menu_widget_activation(&ev) {
                if let Some(game) = self.game.as_mut() {
                    game.menu_click(
                        petramond_world::gui_state::MenuSlot::Widget(petramond_world::gui_state::intern_str(id)),
                        petramond_world::gui_state::PointerButton::Primary,
                        modifier_shift,
                        false,
                    );
                }
                continue;
            }
            match ev {
                petramond_ui::UiEvent::SlotClick {
                    role,
                    index,
                    button,
                    shift,
                } => {
                    let Some(slot) =
                        petramond::gui::Role::from_key(&role).and_then(|r| r.menu_slot(index as usize))
                    else {
                        continue;
                    };
                    let button = to_button(button);
                    let cursor_has_stack = self.game.as_ref().is_some_and(|g| g.cursor_has_stack());
                    let gather =
                        self.gui_router
                            .doc_gather(slot, button, shift, now, cursor_has_stack);
                    if let Some(game) = self.game.as_mut() {
                        game.menu_click(slot, button, shift, gather);
                    }
                }
                petramond_ui::UiEvent::SlotDrag { slots, button } => {
                    self.gui_router.reset_click_streak();
                    let slots = slots
                        .into_iter()
                        .filter_map(|(role, index)| {
                            petramond::gui::Role::from_key(&role)
                                .and_then(|role| role.menu_slot(index as usize))
                        })
                        .collect();
                    if let Some(game) = self.game.as_mut() {
                        game.menu_drag(kind, slots, to_button(button));
                    }
                }
                petramond_ui::UiEvent::ClickOutside { button } => {
                    self.gui_router.reset_click_streak();
                    if let Some(game) = self.game.as_mut() {
                        use petramond::net::protocol::ThrowAmount;
                        game.throw_cursor(match to_button(button) {
                            petramond_world::gui_state::PointerButton::Primary => ThrowAmount::All,
                            petramond_world::gui_state::PointerButton::Secondary => ThrowAmount::One,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}
