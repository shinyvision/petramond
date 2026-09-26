//! Title screen controller: Start Game → world select; Connect to Server →
//! the connect screen; Quit.

use super::{ScreenCtx, ShellCommand};
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState};

pub(super) fn populate(_ctx: &ScreenCtx, _state: &mut UiState) {}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "start" => start(ctx),
            "connect" => ctx.request(ShellCommand::OpenConnectServer),
            "options" => ctx.request(ShellCommand::Push(AppScreen::Options)),
            "quit" => ctx.request(ShellCommand::Quit),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => start(ctx),
        _ => {}
    }
}

fn start(ctx: &mut ScreenCtx) {
    ctx.shell.refresh_worlds();
    ctx.shell.select_world(None);
    ctx.goto(AppScreen::WorldSelect);
}
