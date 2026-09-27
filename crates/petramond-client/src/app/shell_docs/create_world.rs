use super::mods_tab;
use super::{ScreenCtx, ShellCommand};
use crate::app::shell_state::SettingsTab;
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

fn name_taken(ctx: &ScreenCtx, name: &str) -> bool {
    petramond::save::world_exists(name)
        || ctx
            .shell
            .worlds()
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(name))
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    for key in ["create_name", "create_seed"] {
        if state.get(key).is_none() {
            state.set(key, UiValue::Str(String::new()));
        }
    }
    let name = state.get_str("create_name").unwrap_or("").trim().to_owned();
    let exists = !name.is_empty() && name_taken(ctx, &name);
    state.set("name_exists", UiValue::Bool(exists));
    state.set("can_create", UiValue::Bool(!name.is_empty() && !exists));
    let Some(session) = ctx.shell.create_world() else {
        return;
    };
    mods_tab::populate_tabs(session.tab, state);
    mods_tab::populate(&session.rows, &session.settings, session.selected, state);
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::TabSelect { id, index } if id == "tabs" => {
            if let Some(session) = ctx.shell.create_world_mut() {
                session.tab = SettingsTab::from_index(index);
            }
        }
        UiEvent::Toggle {
            id,
            item: Some(row),
            ..
        } if id == "mod_on" => ctx.shell.toggle_create_world_row(row as usize),
        UiEvent::ListSelect { id, index } if id == "mods" => {
            if let Some(session) = ctx.shell.create_world_mut() {
                session.selected = index as usize;
            }
        }
        UiEvent::TextChanged { id, text } => {
            ctx.ui.state_mut().set(id, UiValue::Str(text));
        }
        UiEvent::Submit { .. } => create(ctx),
        UiEvent::Click { id, .. } => match id.as_str() {
            "create" => create(ctx),
            "cancel" => {
                ctx.shell.close_page();
                ctx.goto(AppScreen::WorldSelect);
            }
            _ => {}
        },
        UiEvent::Key { key, .. } => match key {
            NavKey::Left | NavKey::Right => {
                if let Some(session) = ctx.shell.create_world_mut() {
                    session.tab = match key {
                        NavKey::Left => SettingsTab::World,
                        _ => SettingsTab::Mods,
                    };
                }
            }
            NavKey::Enter => {
                if let Some((row, SettingsTab::Mods)) =
                    ctx.shell.create_world().map(|s| (s.selected, s.tab))
                {
                    ctx.shell.toggle_create_world_row(row);
                }
            }
            NavKey::Up => move_selection(ctx, -1),
            NavKey::Down => move_selection(ctx, 1),
            _ => {}
        },
        _ => {}
    }
}

fn create(ctx: &mut ScreenCtx) {
    let name = ctx
        .ui
        .state_mut()
        .get_str("create_name")
        .unwrap_or("")
        .trim()
        .to_owned();
    if name.is_empty() || name_taken(ctx, &name) {
        return;
    }
    let seed_text = ctx
        .ui
        .state_mut()
        .get_str("create_seed")
        .unwrap_or("")
        .trim()
        .to_owned();
    if let Err(e) = petramond::save::write_world_metadata(&name) {
        log::warn!("could not write world metadata for '{name}': {e}");
    }
    let dir_name = petramond::save::dir_name_for(&name);
    if let Some(session) = ctx.shell.take_create_world() {
        if let Err(e) = petramond::save::write_world_settings(&dir_name, &session.settings) {
            log::warn!("could not write settings.json for new world '{name}': {e}");
        }
        if let Err(e) =
            petramond::save::write_world_mod_baseline(&dir_name, &session.settings.disabled_mods)
        {
            log::warn!("could not write mods.json for new world '{name}': {e}");
        }
    }
    let seed = if seed_text.is_empty() {
        petramond::save::random_seed()
    } else {
        petramond::save::seed_from_text(&seed_text)
    };
    ctx.request(ShellCommand::StartGame { dir_name, seed });
}

fn move_selection(ctx: &mut ScreenCtx, step: i32) {
    let Some(session) = ctx.shell.create_world_mut() else {
        return;
    };
    if session.tab != SettingsTab::Mods {
        return;
    }
    mods_tab::move_selection(&mut session.selected, session.rows.len(), step);
}
