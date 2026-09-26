//! Sleep overlay controller: the "Leave bed" button cancels the sleep (ESC
//! stays on the global close-screen control path, which does the same). The
//! darkening backdrop is the host dim quad, driven by the tick-owned sleep
//! progress. With other players connected, an "x/y players sleeping" line
//! (the server's headcount) shows who the morning skip is still waiting on;
//! hidden in single-player.

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
