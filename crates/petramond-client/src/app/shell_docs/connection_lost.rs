//! Disconnected screen controller: shows why the session ended (the game is
//! already torn down by the time this screen is up); OK/Enter → title.

use crate::app::{App, AppScreen};
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    state.set(
        "disconnect_message",
        UiValue::Str(app.disconnect_message.clone()),
    );
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } if id == "ok" => to_title(app),
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => to_title(app),
        _ => {}
    }
}

fn to_title(app: &mut App) {
    app.screen = AppScreen::Title;
    app.pointer.release_for_menu();
}
