//! Account controller: who this client is signed in as on petramond.com,
//! sign in and sign out. Back returns to whichever screen opened it.

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
    // Two bools, not one negated bind: the document runtime resolves a bind key,
    // never an expression (the pause menu's lan_open/lan_closed pair is the
    // precedent).
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

/// The accent line: who this is.
fn name_line(signed_in: Option<&SavedSignIn>) -> String {
    match signed_in {
        Some(saved) => saved.username.clone(),
        None => "Not signed in".to_owned(),
    }
}

/// The muted line under the name. SHORT on purpose, in both states: the label
/// does not wrap — the panel has no spare row at the smallest viewport — so
/// anything wider than the inset ellipsizes away mid-sentence. The account's
/// username is the line above, so this one does not repeat it.
/// `the_account_lines_fit_their_inset` is what keeps them honest.
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
