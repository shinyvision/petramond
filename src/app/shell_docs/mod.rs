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
use crate::audio::Sound;
use crate::gui::GuiKind;
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
            .with_prepare(|app| {
                let icons = world_settings::extra_images(app);
                app.ui.set_extra_images(&icons);
                true
            }),
        GuiKind::CreateWorld => C::screen(create_world::populate, create_world::handle),
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
    table: &crate::controls::ActionTable,
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

fn is_shell_activation(ev: &petramond_ui::UiEvent) -> bool {
    matches!(
        ev,
        petramond_ui::UiEvent::Click { .. } | petramond_ui::UiEvent::Toggle { .. }
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
    /// [`crate::gui::MenuSlot`] clicks — the same deterministic path the
    /// legacy hit-test used. Off-panel presses throw the cursor stack.
    pub(super) fn drive_doc_menu(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        self.ui.ensure_active(kind);
        let crafting_station = match kind {
            GuiKind::Inventory => Some(crate::crafting::CraftingStation::Inventory),
            GuiKind::CraftingTable => Some(crate::crafting::CraftingStation::CraftingTable),
            _ => None,
        };
        if let (Some(station), Some(game)) = (crafting_station, self.game.as_ref()) {
            let mut state = std::mem::take(self.ui.state_mut());
            self.crafting_browser.populate(game, station, &mut state);
            *self.ui.state_mut() = state;
        }
        if let Some(game) = self.game.as_ref() {
            let menu = game.menu_read_model();
            let furnace = menu.furnace;
            let gui_state = menu.gui_state;
            let state = self.ui.state_mut();
            if let Some(f) = furnace {
                state.set("cook01", petramond_ui::UiValue::F32(f.cook01));
                state.set("burn01", petramond_ui::UiValue::F32(f.burn01));
            }
            if let Some(map) = gui_state {
                for (key, value) in map.iter() {
                    let v = match value {
                        crate::gui::GuiValue::F32(v) => petramond_ui::UiValue::F32(*v),
                        crate::gui::GuiValue::I32(v) => petramond_ui::UiValue::I32(*v),
                        crate::gui::GuiValue::Str(s) => petramond_ui::UiValue::Str(s.clone()),
                    };
                    state.set(key.clone(), v);
                }
            }
        }
        self.ui.frame(kind, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        let modifier_shift = self.modifiers.shift;
        let to_button = |b: petramond_ui::PointerButton| match b {
            petramond_ui::PointerButton::Primary => crate::controls::PointerButton::Primary,
            petramond_ui::PointerButton::Secondary => crate::controls::PointerButton::Secondary,
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
            match ev {
                // Widget (mod GUI button) clicks: primary only, like the
                // legacy dispatch.
                petramond_ui::UiEvent::Click {
                    id,
                    button: petramond_ui::PointerButton::Primary,
                    ..
                } => {
                    if let Some(game) = self.game.as_mut() {
                        game.menu_click(
                            crate::gui::MenuSlot::Widget(crate::gui::intern_str(&id)),
                            crate::controls::PointerButton::Primary,
                            modifier_shift,
                            false,
                        );
                    }
                }
                petramond_ui::UiEvent::SlotClick {
                    role,
                    index,
                    button,
                    shift,
                } => {
                    let Some(slot) =
                        crate::gui::Role::from_key(&role).and_then(|r| r.menu_slot(index as usize))
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
                            crate::gui::Role::from_key(&role)
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
                        use crate::net::protocol::ThrowAmount;
                        game.throw_cursor(match to_button(button) {
                            crate::controls::PointerButton::Primary => ThrowAmount::All,
                            crate::controls::PointerButton::Secondary => ThrowAmount::One,
                        });
                    }
                }
                _ => {}
            }
        }
    }
}
