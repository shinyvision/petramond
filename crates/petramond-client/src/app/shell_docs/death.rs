//! Death screen controller: respawn (latched to the tick — the teleport and
//! health restore are simulation mutations) or save-and-quit. ESC does not
//! close this screen; death is only left through these buttons.

use super::{ScreenCtx, ShellCommand};
use petramond_ui::{UiEvent, UiState};

pub(super) fn populate(_ctx: &ScreenCtx, _state: &mut UiState) {}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        match id.as_str() {
            "respawn" => ctx.request(ShellCommand::Respawn),
            "save_quit" => ctx.request(ShellCommand::SaveAndQuitToTitle),
            _ => {}
        }
    }
}
