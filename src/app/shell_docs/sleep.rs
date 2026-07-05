//! Sleep overlay controller: the "Leave bed" button cancels the sleep (ESC
//! stays on the global close-screen control path, which does the same). The
//! darkening backdrop is the host dim quad, driven by the tick-owned sleep
//! progress in `drive_doc_ui`.

use crate::app::App;
use llama_ui::{UiEvent, UiState};

pub(super) fn populate(_app: &App, _state: &mut UiState) {}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if let UiEvent::Click { id, .. } = ev {
        if id == "leave_bed" {
            app.cancel_sleep();
        }
    }
}
