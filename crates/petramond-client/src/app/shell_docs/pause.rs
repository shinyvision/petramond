//! Pause screen controller: resume, options, host-only Open to LAN + Save
//! and Quit, remote-only Disconnect. Enter resumes (ESC stays on the global
//! close-screen control path).

use super::{ScreenCtx, ShellCommand};
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let is_remote = ctx.session.is_remote;
    let lan_open = ctx.session.lan_port.is_some();
    state.set("is_remote", UiValue::Bool(is_remote));
    state.set("is_host", UiValue::Bool(!is_remote));
    state.set("lan_open", UiValue::Bool(lan_open));
    state.set("lan_closed", UiValue::Bool(!is_remote && !lan_open));
    state.set(
        "lan_status",
        UiValue::Str(match ctx.session.lan_port {
            Some(port) => format!("Open on port {port}"),
            None => String::new(),
        }),
    );
    let error = ctx.session.lan_error.unwrap_or_default().to_owned();
    state.set("has_lan_error", UiValue::Bool(!error.is_empty()));
    state.set("lan_error", UiValue::Str(error));
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "resume" => ctx.request(ShellCommand::ResumeGame),
            "options" => ctx.request(ShellCommand::Push(AppScreen::Options)),
            "open_lan" => ctx.request(ShellCommand::OpenLan),
            "disconnect" => ctx.request(ShellCommand::DisconnectToTitle),
            "save_quit" => ctx.request(ShellCommand::SaveAndQuitToTitle),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.request(ShellCommand::ResumeGame),
        _ => {}
    }
}
