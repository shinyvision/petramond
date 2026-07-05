//! Death screen controller: respawn (latched to the tick — the teleport and
//! health restore are simulation mutations) or save-and-quit. ESC does not
//! close this screen; death is only left through these buttons.

use crate::app::App;
use llama_ui::{UiEvent, UiState};

pub(super) fn populate(_app: &App, _state: &mut UiState) {}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        match id.as_str() {
            "respawn" => {
                if let Some(game) = app.game.as_mut() {
                    game.request_respawn();
                }
            }
            "save_quit" => app.save_and_quit_to_title(),
            _ => {}
        }
    }
}
