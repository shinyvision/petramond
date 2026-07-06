//! Controllers for document-backed shell screens.
//!
//! Each screen is a GUI document (`assets/ui/documents/<name>.gui.json`) plus
//! one controller module here: `populate` writes the screen's dynamic values
//! into the [`llama_ui::UiState`] the document binds, and `handle` maps the
//! frame's resolved [`llama_ui::UiEvent`]s to app actions (screen
//! transitions, world I/O). A screen routes through here exactly when
//! [`App::doc_ui_kind`] maps it and its document loads.
//!
//! Runs from [`App::update`], never from render — controllers mutate
//! app-shell state, and presentation only hands the already-built draw list
//! to the renderer.

mod create_world;
mod death;
mod delete_world;
mod pause;
mod sleep;
mod title;
mod world_select;
mod world_settings;

use super::App;
use crate::audio::Sound;
use crate::gui::GuiKind;
use llama_ui::UiState;

/// Split-borrow helper: controllers read `&App` while writing the UI state.
fn with_state(app: &mut App, f: impl FnOnce(&App, &mut UiState)) {
    let mut state = std::mem::take(app.ui.state_mut());
    f(app, &mut state);
    *app.ui.state_mut() = state;
}

fn is_shell_activation(ev: &llama_ui::UiEvent) -> bool {
    matches!(
        ev,
        llama_ui::UiEvent::Click { .. } | llama_ui::UiEvent::Toggle { .. }
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
                GuiKind::Pause => pause::handle(self, ev),
                GuiKind::Sleep => sleep::handle(self, ev),
                GuiKind::Death => death::handle(self, ev),
                _ => {}
            }
        }
    }

    /// Drive one frame of a document-backed GAME MENU (mod GUIs and
    /// containers): bound values come from the tick-owned GUI state map and
    /// the container views; slot + widget clicks latch to the tick as
    /// [`crate::gui::MenuSlot`] clicks — the same deterministic path the
    /// legacy hit-test used. Off-panel presses throw the cursor stack.
    pub(super) fn drive_doc_menu(&mut self, kind: GuiKind, screen: (u32, u32), now: f64) {
        self.ui.ensure_active(kind);
        if let Some(game) = self.game.as_ref() {
            let menu = game.menu_read_model();
            let furnace = menu.furnace;
            let gui_state = menu.gui_state;
            let state = self.ui.state_mut();
            if let Some(f) = furnace {
                state.set("cook01", llama_ui::UiValue::F32(f.cook01));
                state.set("burn01", llama_ui::UiValue::F32(f.burn01));
            }
            if let Some(map) = gui_state {
                for (key, value) in map.iter() {
                    let v = match value {
                        crate::gui::GuiValue::F32(v) => llama_ui::UiValue::F32(*v),
                        crate::gui::GuiValue::I32(v) => llama_ui::UiValue::I32(*v),
                        crate::gui::GuiValue::Str(s) => llama_ui::UiValue::Str(s.clone()),
                    };
                    state.set(key.clone(), v);
                }
            }
        }
        self.ui.frame(kind, screen, now, Some([0.0, 0.0, 0.0, 0.6]));
        let modifier_shift = self.modifiers.shift;
        let to_button = |b: llama_ui::PointerButton| match b {
            llama_ui::PointerButton::Primary => crate::controls::PointerButton::Primary,
            llama_ui::PointerButton::Secondary => crate::controls::PointerButton::Secondary,
        };
        for ev in self.ui.take_events() {
            match ev {
                // Widget (mod GUI button) clicks: primary only, like the
                // legacy dispatch.
                llama_ui::UiEvent::Click {
                    id,
                    button: llama_ui::PointerButton::Primary,
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
                llama_ui::UiEvent::SlotClick {
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
                llama_ui::UiEvent::ClickOutside { button } => {
                    self.gui_router.reset_click_streak();
                    if let Some(game) = self.game.as_mut() {
                        match to_button(button) {
                            crate::controls::PointerButton::Primary => game.throw_cursor_stack(),
                            crate::controls::PointerButton::Secondary => game.throw_cursor_one(),
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
