//! Options root controller: the Sound / Controls / Graphics category buttons
//! plus Back (to the title or the pause menu — wherever the flow began).

use crate::app::{App, AppScreen};
use petramond_ui::{UiEvent, UiState};

pub(super) fn populate(app: &App, state: &mut UiState) {
    super::populate_options_chrome(app, state);
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
