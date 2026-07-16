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
use petramond_ui::UiState;

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
        match kind {
            GuiKind::Demo => super::ui_runtime::demo::populate(self.ui.state_mut()),
            GuiKind::Title => with_state(self, title::populate),
            GuiKind::WorldSelect => with_state(self, world_select::populate),
            GuiKind::WorldSettings => {
                let icons = world_settings::extra_images(self);
                self.ui.set_extra_images(&icons);
                with_state(self, world_settings::populate);
            }
            GuiKind::CreateWorld => with_state(self, create_world::populate),
            GuiKind::DeleteWorld => with_state(self, delete_world::populate),
            GuiKind::ConnectServer => {
                // Per-frame prep: consume the connect worker's outcomes BEFORE
                // binding. A terminal outcome switches screens — skip the rest
                // of this frame rather than draw the stale connect UI.
                self.poll_connect_worker();
                if !matches!(self.screen, AppScreen::ConnectServer) {
                    return;
                }
                with_state(self, connect_server::populate);
            }
            GuiKind::ModsMissing => with_state(self, mods_missing::populate),
            GuiKind::ConnectionLost => with_state(self, connection_lost::populate),
            GuiKind::Options => with_state(self, options::populate),
            GuiKind::OptionsSound => with_state(self, options_sound::populate),
            GuiKind::OptionsControls => with_state(self, options_controls::populate),
            GuiKind::OptionsGraphics => with_state(self, options_graphics::populate),
            GuiKind::Pause => with_state(self, pause::populate),
            GuiKind::Sleep => with_state(self, sleep::populate),
            GuiKind::Death => with_state(self, death::populate),
            _ => {}
        }
        // Screens over live gameplay dim the world behind them: a flat menu
        // dim for pause, the tick-driven darkening fade for sleep, and a
        // subtle red tint for death.
        let dim = match kind {
            GuiKind::Pause => Some([0.0, 0.0, 0.0, 0.6]),
            // Options screens over a paused/running game dim like the pause
            // menu; from the title flow the document's own backdrop shows.
            GuiKind::Options
            | GuiKind::OptionsSound
            | GuiKind::OptionsControls
            | GuiKind::OptionsGraphics
                if self.game.is_some() =>
            {
                Some([0.0, 0.0, 0.0, 0.6])
            }
            GuiKind::Sleep => {
                let progress = self
                    .game
                    .as_ref()
                    .and_then(|g| g.sleep_progress01())
                    .unwrap_or(1.0);
                Some([0.0, 0.0, 0.0, 0.25 + 0.75 * progress])
            }
            GuiKind::Death => Some([0.35, 0.02, 0.02, 0.40]),
            _ => None,
        };
        self.ui.frame(kind, screen, now, dim);
        let events = self.ui.take_events();
        for ev in events {
            if is_shell_activation(&ev) {
                self.audio.play(Sound::UiClick);
            }
            match kind {
                GuiKind::Demo => {
                    super::ui_runtime::demo::apply_one(self.ui.state_mut(), &ev);
                }
                GuiKind::Title => title::handle(self, ev),
                GuiKind::WorldSelect => world_select::handle(self, ev),
                GuiKind::WorldSettings => world_settings::handle(self, ev),
                GuiKind::CreateWorld => create_world::handle(self, ev),
                GuiKind::DeleteWorld => delete_world::handle(self, ev),
                GuiKind::ConnectServer => connect_server::handle(self, ev),
                GuiKind::ModsMissing => mods_missing::handle(self, ev),
                GuiKind::ConnectionLost => connection_lost::handle(self, ev),
                GuiKind::Options => options::handle(self, ev),
                GuiKind::OptionsSound => options_sound::handle(self, ev),
                GuiKind::OptionsControls => options_controls::handle(self, ev),
                GuiKind::OptionsGraphics => options_graphics::handle(self, ev),
                GuiKind::Pause => pause::handle(self, ev),
                GuiKind::Sleep => sleep::handle(self, ev),
                GuiKind::Death => death::handle(self, ev),
                _ => {}
            }
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
