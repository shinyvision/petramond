//! Sleep overlay controller: the "Leave bed" button cancels the sleep (ESC
//! stays on the global close-screen control path, which does the same). The
//! darkening backdrop is the host dim quad, driven by the tick-owned sleep
//! progress in `drive_doc_ui`. With other players connected, an "x/y players
//! sleeping" line (from the replicated player rows + self) shows who the
//! morning skip is still waiting on; hidden in single-player.

use crate::app::App;
use petramond_ui::{UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    let (sleeping, total) = app
        .game
        .as_ref()
        .map(|g| g.sleeping_player_counts())
        .unwrap_or((0, 1));
    state.set("sleep_count_visible", UiValue::Bool(total > 1));
    state.set(
        "sleep_count",
        UiValue::Str(format!("{sleeping}/{total} players sleeping")),
    );
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        if id == "leave_bed" {
            app.cancel_sleep();
        }
    }
}
