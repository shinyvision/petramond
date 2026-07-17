//! World Settings controller: a tabbed screen — the World tab (per-world
//! options; currently empty) and the Mods tab (per-world pack toggles, each
//! writing `settings.json` immediately; applies on next world open) — plus
//! the header's inline world-rename editor and the shared Back/Delete footer.

use super::mods_tab;
use crate::app::shell::SettingsTab;
use crate::app::{App, AppScreen};
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    let Some(session) = app.world_settings.as_ref() else {
        return;
    };
    state.set("world_name", UiValue::Str(session.world_name.clone()));
    state.set("renaming", UiValue::Bool(session.renaming));
    state.set("not_renaming", UiValue::Bool(!session.renaming));
    mods_tab::populate_tabs(session.tab, state);
    mods_tab::populate(&session.rows, &session.settings, session.selected, state);
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::TabSelect { id, index } if id == "tabs" => {
            if let Some(session) = app.world_settings.as_mut() {
                session.tab = SettingsTab::from_index(index);
            }
        }
        UiEvent::Toggle {
            id,
            item: Some(row),
            ..
        } if id == "mod_on" => app.toggle_world_settings_row(row as usize),
        UiEvent::ListSelect { id, index } if id == "mods" => {
            if let Some(session) = app.world_settings.as_mut() {
                session.selected = index as usize;
            }
        }
        UiEvent::TextChanged { id, text } if id == "rename_input" => {
            app.ui.state_mut().set("rename_text", UiValue::Str(text));
        }
        UiEvent::Submit { id, text } if id == "rename_input" => apply_rename(app, &text),
        UiEvent::Click { id, .. } => match id.as_str() {
            "rename" => {
                let name = app
                    .world_settings
                    .as_mut()
                    .map(|s| {
                        s.renaming = true;
                        s.world_name.clone()
                    })
                    .unwrap_or_default();
                app.ui
                    .state_mut()
                    .set("rename_text", UiValue::Str(name.clone()));
                app.ui.focus_text_input("rename_input", &name, 48);
            }
            "rename_confirm" => {
                let text = app
                    .ui
                    .state_mut()
                    .get_str("rename_text")
                    .unwrap_or_default()
                    .to_owned();
                apply_rename(app, &text);
            }
            "back" => {
                app.world_settings = None;
                app.screen = AppScreen::WorldSelect;
                app.pointer.release_for_menu();
            }
            "delete_world" => {
                app.world_settings = None;
                app.open_delete_world_confirm();
            }
            _ => {}
        },
        UiEvent::Key { key, .. } => match key {
            NavKey::Escape => {
                if let Some(session) = app.world_settings.as_mut() {
                    session.renaming = false;
                }
            }
            NavKey::Left | NavKey::Right => {
                if let Some(session) = app.world_settings.as_mut() {
                    session.tab = match key {
                        NavKey::Left => SettingsTab::World,
                        _ => SettingsTab::Mods,
                    };
                }
            }
            NavKey::Enter => {
                if let Some((row, SettingsTab::Mods)) =
                    app.world_settings.as_ref().map(|s| (s.selected, s.tab))
                {
                    app.toggle_world_settings_row(row);
                }
            }
            NavKey::Delete => {
                app.world_settings = None;
                app.open_delete_world_confirm();
            }
            NavKey::Up => move_selection(app, -1),
            NavKey::Down => move_selection(app, 1),
            _ => {}
        },
        _ => {}
    }
}

fn apply_rename(app: &mut App, new_name: &str) {
    let Some(session) = app.world_settings.as_mut() else {
        return;
    };
    let new_name = new_name.trim();
    if new_name.is_empty() {
        session.renaming = false;
        return;
    }
    match crate::save::rename_world(&session.dir_name, new_name) {
        Ok(()) => {
            session.world_name = new_name.to_owned();
            session.renaming = false;
            app.refresh_worlds();
        }
        Err(e) => {
            log::warn!("could not rename world '{}': {e}", session.world_name);
            session.renaming = false;
        }
    }
}

fn move_selection(app: &mut App, step: i32) {
    let Some(session) = app.world_settings.as_mut() else {
        return;
    };
    if session.tab != SettingsTab::Mods {
        return;
    }
    mods_tab::move_selection(&mut session.selected, session.rows.len(), step);
}
