//! Options root controller: the Sound / Controls / Graphics category buttons
//! plus Back (to the title or the pause menu — wherever the flow began).

use crate::app::{App, AppScreen};
use petramond_ui::{UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    // Title flow shows the screenshot backdrop; over a paused game the host
    // dim does the work instead.
    state.set("show_backdrop", UiValue::Bool(app.game.is_none()));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        match id.as_str() {
            "sound" => app.screen = AppScreen::OptionsSound,
            "controls" => app.screen = AppScreen::OptionsControls,
            "graphics" => app.screen = AppScreen::OptionsGraphics,
            "back" => app.close_options_root(),
            _ => {}
        }
    }
}
