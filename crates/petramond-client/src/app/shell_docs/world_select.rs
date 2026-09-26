//! World-select controller: pick a world, play it (double-click/Enter),
//! create a new one, open per-world settings (Delete key follows the button),
//! back to title.

use super::{ScreenCtx, ShellCommand};
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let rows: Vec<UiMap> = ctx
        .shell
        .worlds()
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
        UiValue::I32(ctx.shell.selected_world().map(|i| i as i32).unwrap_or(-1)),
    );
    state.set(
        "has_selection",
        UiValue::Bool(ctx.shell.selected_world_info().is_some()),
    );
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::ListSelect { id, index } if id == "worlds" => {
            ctx.shell.select_world(Some(index as usize));
        }
        UiEvent::ListActivate { id, index } if id == "worlds" => {
            ctx.shell.select_world(Some(index as usize));
            ctx.request(ShellCommand::PlaySelectedWorld);
        }
        UiEvent::Click { id, .. } => match id.as_str() {
            "play" => ctx.request(ShellCommand::PlaySelectedWorld),
            "create" => open_create_world(ctx),
            "settings" => open_world_settings(ctx),
            "back" => ctx.goto(AppScreen::Title),
            _ => {}
        },
        UiEvent::Key { key, .. } => match key {
            NavKey::Enter => ctx.request(ShellCommand::PlaySelectedWorld),
            NavKey::Delete => open_world_settings(ctx),
            NavKey::Up => ctx.shell.move_world_selection(-1),
            NavKey::Down => ctx.shell.move_world_selection(1),
            _ => {}
        },
        _ => {}
    }
}

/// Open the Create World page with a fresh session (all mods enabled).
fn open_create_world(ctx: &mut ScreenCtx) {
    ctx.shell.open_create_world();
    ctx.goto(AppScreen::CreateWorld);
}

/// Open the World Settings page for the selected world, if any.
fn open_world_settings(ctx: &mut ScreenCtx) {
    if ctx.shell.open_world_settings() {
        ctx.goto(AppScreen::WorldSettings);
    }
}
