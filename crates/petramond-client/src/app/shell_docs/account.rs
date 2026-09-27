use super::{ScreenCtx, ShellCommand};
use petramond::account::SavedSignIn;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn prepare(ctx: &mut ScreenCtx) -> bool {
    ctx.shell.account.poll();
    true
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let account = &ctx.shell.account;
    let signed_in = account.saved.as_ref();
    state.set("is_signed_in", UiValue::Bool(signed_in.is_some()));
    state.set("signed_out", UiValue::Bool(signed_in.is_none()));
    state.set("account_name", UiValue::Str(name_line(signed_in)));
    state.set("account_detail", UiValue::Str(detail_line(signed_in)));
    let busy = account.busy.unwrap_or("");
    state.set("working", UiValue::Bool(!busy.is_empty()));
    state.set("working_text", UiValue::Str(busy.to_owned()));
    state.set("has_status", UiValue::Bool(!account.status.is_empty()));
    state.set("status_text", UiValue::Str(account.status.clone()));
}

fn name_line(signed_in: Option<&SavedSignIn>) -> String {
    match signed_in {
        Some(saved) => saved.username.clone(),
        None => "Not signed in".to_owned(),
    }
}

fn detail_line(signed_in: Option<&SavedSignIn>) -> String {
    match signed_in {
        Some(_) => "Signed in to petramond.com".to_owned(),
        None => "Sign in to play multiplayer.".to_owned(),
    }
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "sign_in" => ctx.request(ShellCommand::OpenAccountSignIn),
            "sign_out" => ctx.request(ShellCommand::AccountSignOut),
            "back" => ctx.request(ShellCommand::LeaveAccount),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.request(ShellCommand::LeaveAccount),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
