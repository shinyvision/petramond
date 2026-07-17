//! Create-world controller: a tabbed screen — the World tab (name + optional
//! seed; the document's text inputs own the editing, mirrored into bound
//! state so `Create` enablement and the duplicate-name warning stay live) and
//! the Mods tab (pick the new world's enabled packs; buffered in the session
//! and written as the world's `settings.json` on Create).

use super::mods_tab;
use crate::app::shell::SettingsTab;
use crate::app::{App, AppScreen};
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

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
    let Some(session) = app.create_world.as_ref() else {
        return;
    };
    mods_tab::populate_tabs(session.tab, state);
    mods_tab::populate(&session.rows, &session.settings, session.selected, state);
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::TabSelect { id, index } if id == "tabs" => {
            if let Some(session) = app.create_world.as_mut() {
                session.tab = SettingsTab::from_index(index);
            }
        }
        UiEvent::Toggle {
            id,
            item: Some(row),
            ..
        } if id == "mod_on" => app.toggle_create_world_row(row as usize),
        UiEvent::ListSelect { id, index } if id == "mods" => {
            if let Some(session) = app.create_world.as_mut() {
                session.selected = index as usize;
            }
        }
        UiEvent::TextChanged { id, text } => {
            app.ui.state_mut().set(id, UiValue::Str(text));
        }
        UiEvent::Submit { .. } => create(app),
        UiEvent::Click { id, .. } => match id.as_str() {
            "create" => create(app),
            "cancel" => {
                app.create_world = None;
                app.screen = AppScreen::WorldSelect;
                app.pointer.release_for_menu();
            }
            _ => {}
        },
        UiEvent::Key { key, .. } => match key {
            NavKey::Left | NavKey::Right => {
                if let Some(session) = app.create_world.as_mut() {
                    session.tab = match key {
                        NavKey::Left => SettingsTab::World,
                        _ => SettingsTab::Mods,
                    };
                }
            }
            NavKey::Enter => {
                if let Some((row, SettingsTab::Mods)) =
                    app.create_world.as_ref().map(|s| (s.selected, s.tab))
                {
                    app.toggle_create_world_row(row);
                }
            }
            NavKey::Up => move_selection(app, -1),
            NavKey::Down => move_selection(app, 1),
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
    let dir_name = crate::save::dir_name_for(&name);
    if let Some(session) = app.create_world.take() {
        if let Err(e) = crate::save::write_world_settings(&dir_name, &session.settings) {
            log::warn!("could not write settings.json for new world '{name}': {e}");
        }
    }
    let seed = if seed_text.is_empty() {
        crate::save::random_seed()
    } else {
        crate::save::seed_from_text(&seed_text)
    };
    app.start_game(&dir_name, seed);
}

fn move_selection(app: &mut App, step: i32) {
    let Some(session) = app.create_world.as_mut() else {
        return;
    };
    if session.tab != SettingsTab::Mods {
        return;
    }
    mods_tab::move_selection(&mut session.selected, session.rows.len(), step);
}
