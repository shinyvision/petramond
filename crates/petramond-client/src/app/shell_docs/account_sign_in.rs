//! The Petramond sign-in form: username-or-email plus password, exchanged for
//! the long-lived rotating token the client stores (`petramond::account`).
//!
//! The password never leaves this screen's worker thread — it is spent on the
//! one request that mints a token and is not kept anywhere.

use super::{ScreenCtx, ShellCommand};
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

/// A sign-in that completed leaves the form, skipping this frame.
pub(super) fn prepare(ctx: &mut ScreenCtx) -> bool {
    if ctx.shell.account.poll() {
        ctx.request(ShellCommand::LeaveAccountSignIn);
        return false;
    }
    true
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let account = &ctx.shell.account;
    for key in ["account_id", "account_password"] {
        if state.get(key).is_none() {
            state.set(key, UiValue::Str(String::new()));
        }
    }
    let identifier = state.get_str("account_id").unwrap_or("").trim().to_owned();
    let password = state.get_str("account_password").unwrap_or("").to_owned();
    let busy = account.busy.unwrap_or("");
    state.set("working", UiValue::Bool(!busy.is_empty()));
    state.set("working_text", UiValue::Str(busy.to_owned()));
    state.set("has_status", UiValue::Bool(!account.status.is_empty()));
    state.set("status_text", UiValue::Str(account.status.clone()));
    state.set(
        "can_submit",
        UiValue::Bool(!account.working() && !identifier.is_empty() && !password.is_empty()),
    );
    state.set("service_line", UiValue::Str(service_line()));
}

/// Which account service the password is about to be sent to. WRAPS in the
/// document, so its length costs the panel a row — the real-copy panel guard in
/// `super::account::tests` is what proves the panel still has one.
pub(in crate::app) fn service_line() -> String {
    format!("Signs in against {}", petramond::account::service_url())
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::TextChanged { id, text } => {
            ctx.shell.account.status.clear();
            ctx.ui.state_mut().set(id, UiValue::Str(text));
        }
        UiEvent::Submit { .. } => ctx.request(ShellCommand::SubmitAccountSignIn),
        UiEvent::Click { id, .. } => match id.as_str() {
            "submit" => ctx.request(ShellCommand::SubmitAccountSignIn),
            "back" => ctx.request(ShellCommand::LeaveAccountSignIn),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.request(ShellCommand::SubmitAccountSignIn),
        _ => {}
    }
}
