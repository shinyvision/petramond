use super::ScreenCtx;
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    state.set(
        "disconnect_message",
        UiValue::Str(ctx.shell.disconnect_message().to_owned()),
    );
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } if id == "ok" => ctx.goto(AppScreen::Title),
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.goto(AppScreen::Title),
        _ => {}
    }
}
