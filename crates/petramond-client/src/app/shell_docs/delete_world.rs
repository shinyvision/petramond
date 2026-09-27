use super::ScreenCtx;
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let name = ctx
        .shell
        .selected_world_info()
        .map(|w| w.name.clone())
        .unwrap_or_else(|| "No world selected".to_owned());
    state.set("world_name", UiValue::Str(name));
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "confirm" => confirm(ctx),
            "cancel" => ctx.goto(AppScreen::WorldSelect),
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => confirm(ctx),
        _ => {}
    }
}

fn confirm(ctx: &mut ScreenCtx) {
    ctx.shell.delete_selected_world();
    ctx.goto(AppScreen::WorldSelect);
}
