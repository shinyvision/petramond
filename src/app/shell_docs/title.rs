//! Title screen controller: Start Game → world select; Connect to Server →
//! the connect screen; Quit.

use crate::app::{App, AppScreen};
use llama_ui::{NavKey, UiEvent, UiState};

pub(super) fn populate(_app: &App, _state: &mut UiState) {}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "start" => start(app),
            "connect" => app.open_connect_server(),
            "quit" => app.quit_requested = true,
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => start(app),
        _ => {}
    }
}

fn start(app: &mut App) {
    app.refresh_worlds();
    app.selected_world = None;
    app.screen = AppScreen::WorldSelect;
    app.pointer.release_for_menu();
}
