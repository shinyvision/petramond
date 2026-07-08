//! Delete-world confirmation controller. Confirm and cancel both return to
//! world select (confirm deletes first), exactly like the legacy screen.

use crate::app::{App, AppScreen};
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    let name = app
        .selected_world
        .and_then(|i| app.worlds.get(i))
        .map(|w| w.name.clone())
        .unwrap_or_else(|| "No world selected".to_owned());
    state.set("world_name", UiValue::Str(name));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } => match id.as_str() {
            "confirm" => app.delete_selected_world(),
            "cancel" => {
                app.screen = AppScreen::WorldSelect;
                app.pointer.release_for_menu();
            }
            _ => {}
        },
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => app.delete_selected_world(),
        _ => {}
    }
}
