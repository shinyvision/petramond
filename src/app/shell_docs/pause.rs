//! Pause screen controller: resume / save-and-quit, Enter resumes (ESC stays
//! on the global close-screen control path).

use crate::app::App;
use llama_ui::{NavKey, UiEvent, UiState};

pub(super) fn populate(_app: &App, _state: &mut UiState) {}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "resume" => app.resume_game(),
            "save_quit" => app.save_and_quit_to_title(),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => app.resume_game(),
        _ => {}
    }
}
