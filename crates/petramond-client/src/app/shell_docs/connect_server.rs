//! Connect to Server controller: address + player-name entry (the document's
//! text inputs own the editing; this controller mirrors their text into bound
//! state), the connect worker's status (progress label, inline failure), and
//! Connect gating. The worker itself lives in `crate::app::connect`.

use super::{ScreenCtx, ShellCommand};
use crate::app::connect::ConnectPhase;
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    for key in ["server_addr", "player_name"] {
        if state.get(key).is_none() {
            state.set(key, UiValue::Str(String::new()));
        }
    }
    let addr = state.get_str("server_addr").unwrap_or("").trim().to_owned();
    let name = state.get_str("player_name").unwrap_or("").trim().to_owned();
    let connect = &ctx.shell.connect;
    let connecting = connect.connecting();
    let label = match &connect.phase {
        ConnectPhase::Connecting { label } => label,
        _ => "",
    };
    let status = match &connect.phase {
        ConnectPhase::Failed { message } => message.clone(),
        _ => String::new(),
    };
    state.set("connecting", UiValue::Bool(connecting));
    state.set("connect_phase", UiValue::Str(label.to_owned()));
    state.set("has_status", UiValue::Bool(!status.is_empty()));
    state.set("status_text", UiValue::Str(status));
    state.set(
        "can_connect",
        UiValue::Bool(!connecting && !addr.is_empty() && !name.is_empty()),
    );
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::TextChanged { id, text } => {
            // Typing after a failure clears the stale error.
            if matches!(ctx.shell.connect.phase, ConnectPhase::Failed { .. }) {
                ctx.shell.connect.phase = ConnectPhase::Editing;
            }
            ctx.ui.state_mut().set(id, UiValue::Str(text));
        }
        UiEvent::Submit { .. } => ctx.request(ShellCommand::BeginConnect),
        UiEvent::Click { id, .. } => match id.as_str() {
            "connect" => ctx.request(ShellCommand::BeginConnect),
            "cancel" => ctx.shell.connect.cancel(),
            "back" => {
                ctx.shell.connect.cancel();
                ctx.goto(AppScreen::Title);
            }
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.request(ShellCommand::BeginConnect),
        _ => {}
    }
}
