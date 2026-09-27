use super::{ScreenCtx, ShellCommand};
use petramond_ui::{UiEvent, UiState, UiValue};

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let (sleeping, total) = ctx.session.sleep_counts;
    state.set("sleep_count_visible", UiValue::Bool(total > 1));
    state.set(
        "sleep_count",
        UiValue::Str(format!("{sleeping}/{total} players sleeping")),
    );
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        if id == "leave_bed" {
            ctx.request(ShellCommand::CancelSleep);
        }
    }
}
