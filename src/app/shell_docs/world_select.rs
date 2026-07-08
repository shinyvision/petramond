//! World-select controller: pick a world, play it (double-click/Enter),
//! create a new one, open per-world settings (Delete key follows the button),
//! back to title.

use crate::app::{App, AppScreen};
use petramond_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

pub(super) fn populate(app: &App, state: &mut UiState) {
    let rows: Vec<UiMap> = app
        .worlds
        .iter()
        .map(|w| {
            let mut m = UiMap::new();
            let name = if w.has_level {
                w.name.clone()
            } else {
                format!("{} (new)", w.name)
            };
            m.insert("name".into(), UiValue::Str(name));
            m
        })
        .collect();
    state.set("no_worlds", UiValue::Bool(rows.is_empty()));
    state.set("worlds", UiValue::List(Arc::new(rows)));
    state.set(
        "world_sel",
        UiValue::I32(app.selected_world.map(|i| i as i32).unwrap_or(-1)),
    );
    state.set(
        "has_selection",
        UiValue::Bool(app.selected_world.is_some_and(|i| i < app.worlds.len())),
    );
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    match ev {
        UiEvent::ListSelect { id, index } if id == "worlds" => {
            app.selected_world = Some(index as usize);
        }
        UiEvent::ListActivate { id, index } if id == "worlds" => {
            app.selected_world = Some(index as usize);
            app.play_selected_world();
        }
        UiEvent::Click { id, .. } => match id.as_str() {
            "play" => app.play_selected_world(),
            "create" => open_create(app),
            "settings" => app.open_world_settings(),
            "back" => {
                app.screen = AppScreen::Title;
                app.pointer.release_for_menu();
            }
            _ => {}
        },
        UiEvent::Key { key, .. } => match key {
            NavKey::Enter => app.play_selected_world(),
            NavKey::Delete => app.open_world_settings(),
            NavKey::Up => move_selection(app, -1),
            NavKey::Down => move_selection(app, 1),
            _ => {}
        },
        _ => {}
    }
}

fn open_create(app: &mut App) {
    app.screen = AppScreen::CreateWorld;
    app.pointer.release_for_menu();
}

fn move_selection(app: &mut App, step: i32) {
    if app.worlds.is_empty() {
        return;
    }
    let next = match app.selected_world {
        Some(i) => (i as i32 + step).clamp(0, app.worlds.len() as i32 - 1) as usize,
        None => 0,
    };
    app.selected_world = Some(next);
}
