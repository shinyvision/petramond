//! Pause screen controller: resume, host-only Open to LAN + Save and Quit,
//! remote-only Disconnect. Enter resumes (ESC stays on the global
//! close-screen control path).

use crate::app::App;
use llama_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    let is_remote = app.game.as_ref().is_some_and(|g| g.is_remote());
    let lan_open = app.lan_port.is_some();
    state.set("is_remote", UiValue::Bool(is_remote));
    state.set("is_host", UiValue::Bool(!is_remote));
    state.set("lan_open", UiValue::Bool(lan_open));
    state.set("lan_closed", UiValue::Bool(!is_remote && !lan_open));
    state.set(
        "lan_status",
        UiValue::Str(match app.lan_port {
            Some(port) => format!("Open on port {port}"),
            None => String::new(),
        }),
    );
    let error = app.lan_error.clone().unwrap_or_default();
    state.set("has_lan_error", UiValue::Bool(!error.is_empty()));
    state.set("lan_error", UiValue::Str(error));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "resume" => app.resume_game(),
            "open_lan" => app.open_lan(),
            "disconnect" => app.disconnect_to_title(),
            "save_quit" => app.save_and_quit_to_title(),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => app.resume_game(),
        _ => {}
    }
}
