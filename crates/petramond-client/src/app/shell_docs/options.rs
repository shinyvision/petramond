//! Options root controller: the Sound / Controls / Graphics category buttons
//! plus Back (to the title or the pause menu — wherever the flow began).

use super::{ScreenCtx, ShellCommand};
use crate::app::AppScreen;
use petramond_ui::{UiEvent, UiState};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    super::populate_options_chrome(ctx, state);
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        // Moving within the options flow keeps the menu pointer as it is.
        match id.as_str() {
            "sound" => ctx.request(ShellCommand::SwitchTo(AppScreen::OptionsSound)),
            "controls" => ctx.request(ShellCommand::SwitchTo(AppScreen::OptionsControls)),
            "graphics" => ctx.request(ShellCommand::SwitchTo(AppScreen::OptionsGraphics)),
            "back" => ctx.request(ShellCommand::CloseOptionsRoot),
            _ => {}
        }
    }
}
