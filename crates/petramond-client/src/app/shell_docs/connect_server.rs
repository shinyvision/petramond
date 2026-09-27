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
    let signed_in = ctx.shell.account.saved.as_ref();
    state.set("signed_in_as", UiValue::Str(identity_line(signed_in)));
    state.set("is_signed_in", UiValue::Bool(signed_in.is_some()));
    state.set("signed_out", UiValue::Bool(signed_in.is_none()));
}

pub(in crate::app) fn identity_line(signed_in: Option<&petramond::account::SavedSignIn>) -> String {
    match signed_in {
        Some(saved) => format!("Signed in as {}", saved.username),
        None => "Not signed in — servers that require an account will refuse".to_owned(),
    }
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::TextChanged { id, text } => {
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
