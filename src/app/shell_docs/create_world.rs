//! Create-world controller: name + optional seed. The document's text inputs
//! own the editing; this controller mirrors their text into bound state so
//! `Create` enablement and the duplicate-name warning stay live.

use crate::app::{App, AppScreen};
use llama_ui::{UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    for key in ["create_name", "create_seed"] {
        if state.get(key).is_none() {
            state.set(key, UiValue::Str(String::new()));
        }
    }
    let name = state.get_str("create_name").unwrap_or("").trim().to_owned();
    // Taken when the directory exists OR any listed world displays the name
    // (renamed worlds keep their original directory).
    let exists = !name.is_empty()
        && (crate::save::world_exists(&name)
            || app
                .worlds
                .iter()
                .any(|w| w.name.eq_ignore_ascii_case(&name)));
    state.set("name_exists", UiValue::Bool(exists));
    state.set("can_create", UiValue::Bool(!name.is_empty() && !exists));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::TextChanged { id, text } => {
            app.ui.state_mut().set(id, UiValue::Str(text));
        }
        UiEvent::Submit { .. } => create(app),
        UiEvent::Click { id, .. } => match id.as_str() {
            "create" => create(app),
            "cancel" => {
                app.screen = AppScreen::WorldSelect;
                app.pointer.release_for_menu();
            }
            _ => {}
        },
        _ => {}
    }
}

fn create(app: &mut App) {
    let name = app
        .ui
        .state_mut()
        .get_str("create_name")
        .unwrap_or("")
        .trim()
        .to_owned();
    let taken = crate::save::world_exists(&name)
        || app
            .worlds
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(&name));
    if name.is_empty() || taken {
        return;
    }
    let seed_text = app
        .ui
        .state_mut()
        .get_str("create_seed")
        .unwrap_or("")
        .trim()
        .to_owned();
    if let Err(e) = crate::save::write_world_metadata(&name) {
        log::warn!("could not write world metadata for '{name}': {e}");
    }
    let seed = if seed_text.is_empty() {
        crate::save::random_seed()
    } else {
        crate::save::seed_from_text(&seed_text)
    };
    app.start_game(&crate::save::dir_name_for(&name), seed);
}
