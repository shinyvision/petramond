//! Options root controller: the Sound / Controls / Graphics category buttons
//! plus Back (to the title or the pause menu — whichever the flow was
//! pushed over).

use super::{ScreenCtx, ShellCommand};
use crate::app::AppScreen;
use petramond_ui::{UiEvent, UiState};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    super::populate_options_chrome(ctx, state);
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        // Categories push over the root, so their Back returns here.
        match id.as_str() {
            "sound" => ctx.request(ShellCommand::Push(AppScreen::OptionsSound)),
            "controls" => ctx.request(ShellCommand::Push(AppScreen::OptionsControls)),
            "graphics" => ctx.request(ShellCommand::Push(AppScreen::OptionsGraphics)),
            "back" => ctx.request(ShellCommand::Back),
            _ => {}
        }
    }
}
