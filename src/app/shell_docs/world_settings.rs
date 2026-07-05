//! World Settings controller: per-world mod pack toggles (each writes
//! `settings.json` immediately; applies on next world open), the relocated
//! Delete World entry, and the header's inline world-rename editor.

use crate::app::{App, AppScreen};
use llama_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};
use std::path::PathBuf;
use std::sync::Arc;

/// Per-row pack icons for the document's `bind.image` — registered as extra
/// images on the UI driver before `populate` runs.
pub(super) fn extra_images(app: &App) -> Vec<(String, PathBuf)> {
    let Some(session) = app.world_settings.as_ref() else {
        return Vec::new();
    };
    session
        .rows
        .iter()
        .zip(crate::assets::packs())
        .filter_map(|(_, pack)| {
            let icon = pack.icon.clone()?;
            Some((icon_name(pack.id.as_deref(), &pack.name), icon))
        })
        .collect()
}

fn icon_name(id: Option<&str>, name: &str) -> String {
    format!("pack_icon:{}", id.unwrap_or(name))
}

pub(super) fn populate(app: &App, state: &mut UiState) {
    let Some(session) = app.world_settings.as_ref() else {
        return;
    };
    state.set("world_name", UiValue::Str(session.world_name.clone()));
    state.set("renaming", UiValue::Bool(session.renaming));
    state.set("not_renaming", UiValue::Bool(!session.renaming));
    let rows: Vec<UiMap> = session
        .rows
        .iter()
        .zip(crate::assets::packs())
        .map(|(pack, asset)| {
            let mut m = UiMap::new();
            m.insert("name".into(), UiValue::Str(pack.name.clone()));
            let version = pack.version.as_ref().map(|v| format!("v{v}"));
            m.insert("has_version".into(), UiValue::Bool(version.is_some()));
            m.insert(
                "version".into(),
                UiValue::Str(version.unwrap_or_default()),
            );
            let desc = pack.summary.clone().unwrap_or_else(|| pack.description.clone());
            m.insert("desc".into(), UiValue::Str(desc));
            let toggleable = pack.id.is_some();
            let enabled = match &pack.id {
                Some(id) => !session.settings.disabled_mods.contains(id),
                None => true,
            };
            m.insert("enabled".into(), UiValue::Bool(enabled));
            m.insert("toggleable".into(), UiValue::Bool(toggleable));
            m.insert("content_only".into(), UiValue::Bool(!toggleable));
            m.insert("has_icon".into(), UiValue::Bool(asset.icon.is_some()));
            m.insert(
                "icon".into(),
                UiValue::Str(icon_name(pack.id.as_deref(), &pack.name)),
            );
            m
        })
        .collect();
    state.set("no_mods", UiValue::Bool(rows.is_empty()));
    state.set("mod_rows", UiValue::List(Arc::new(rows)));
    state.set("mod_sel", UiValue::I32(session.selected as i32));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
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
                app.ui.state_mut().set("rename_text", UiValue::Str(name.clone()));
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
            NavKey::Enter => {
                if let Some(row) = app.world_settings.as_ref().map(|s| s.selected) {
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
    if session.rows.is_empty() {
        return;
    }
    session.selected =
        (session.selected as i32 + step).clamp(0, session.rows.len() as i32 - 1) as usize;
}
