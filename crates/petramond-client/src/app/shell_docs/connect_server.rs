//! Connect to Server controller: address entry (the document's text input owns
//! the editing; this controller mirrors its text into bound state), the signed-in
//! identity line, the connect worker's status (progress label, inline failure),
//! and Connect gating. The worker itself lives in `crate::app::connect`.

use super::{ScreenCtx, ShellCommand};
use crate::app::connect::ConnectPhase;
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    if state.get("server_addr").is_none() {
        state.set("server_addr", UiValue::Str(String::new()));
    }
    let addr = state.get_str("server_addr").unwrap_or("").trim().to_owned();
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
        UiValue::Bool(!connecting && !addr.is_empty()),
    );
    // Who the player will appear as. A server that checks accounts overrides it
    // with the account username, so an unsigned client is told to sign in rather
    // than left to discover it at the end of a join.
    let signed_in = ctx.shell.account.saved.as_ref();
    state.set("signed_in_as", UiValue::Str(identity_line(signed_in)));
    state.set("is_signed_in", UiValue::Bool(signed_in.is_some()));
    state.set("signed_out", UiValue::Bool(signed_in.is_none()));
}

/// Who the player will appear as on the server they are about to join. This one
/// WRAPS in the document, so length costs the panel a row rather than
/// ellipsizing — `the_account_flow_panels_fit_the_smallest_viewport_with_their_real_copy`
/// is what proves the panel still has that row.
pub(in crate::app) fn identity_line(signed_in: Option<&petramond::account::SavedSignIn>) -> String {
    match signed_in {
        Some(saved) => format!("Signed in as {}", saved.username),
        None => "Not signed in — servers that require an account will refuse".to_owned(),
    }
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
            "account" => ctx.request(ShellCommand::OpenAccount(None)),
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
