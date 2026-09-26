//! World Settings controller: a tabbed screen — the World tab (seed + copy,
//! world size, day length, keep inventory, auto-LAN; every change writes
//! `settings.json` immediately, applied on next world open) and the Mods tab
//! (per-world pack toggles, same write policy) — plus the header's inline
//! world-rename editor and the shared Back/Delete footer.

use super::mods_tab;
use super::ScreenCtx;
use crate::app::shell_state::SettingsTab;
use crate::app::AppScreen;
use petramond_ui::{NavKey, UiEvent, UiState, UiValue};

/// Per-frame prep: adopt the off-thread save-dir size once it lands, then the
/// shared pack-icon registration.
pub(super) fn prepare(ctx: &mut ScreenCtx) -> bool {
    ctx.shell.poll_world_size();
    super::pack_icon_prepare(ctx)
}

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let Some(session) = ctx.shell.world_settings() else {
        return;
    };
    state.set("world_name", UiValue::Str(session.world_name.clone()));
    state.set("renaming", UiValue::Bool(session.renaming));
    state.set("not_renaming", UiValue::Bool(!session.renaming));
    state.set("has_seed", UiValue::Bool(session.seed.is_some()));
    state.set(
        "seed_text",
        UiValue::Str(session.seed.map(|s| s.to_string()).unwrap_or_default()),
    );
    state.set(
        "world_size",
        UiValue::Str(
            session
                .size_bytes
                .map(format_size)
                .unwrap_or_else(|| "...".into()),
        ),
    );
    state.set(
        "day_minutes",
        UiValue::F32(session.settings.day_minutes as f32),
    );
    state.set(
        "day_minutes_text",
        UiValue::Str(format!("{} min", session.settings.day_minutes)),
    );
    state.set(
        "keep_inventory",
        UiValue::Bool(session.settings.keep_inventory),
    );
    state.set("auto_lan", UiValue::Bool(session.settings.auto_open_lan));
    mods_tab::populate_tabs(session.tab, state);
    mods_tab::populate(&session.rows, &session.settings, session.selected, state);
}

/// Human size on the KB/MB/GB ladder ("412 KB", "38.2 MB", "1.24 GB").
fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{:.0} KB", (b / KB).ceil())
    }
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::TabSelect { id, index } if id == "tabs" => {
            if let Some(session) = ctx.shell.world_settings_mut() {
                session.tab = SettingsTab::from_index(index);
            }
        }
        UiEvent::Toggle {
            id,
            item: Some(row),
            ..
        } if id == "mod_on" => ctx.shell.toggle_world_settings_row(row as usize),
        UiEvent::Toggle { id, .. } if id == "keep_inventory" => ctx.shell.toggle_keep_inventory(),
        UiEvent::Toggle { id, .. } if id == "auto_lan" => ctx.shell.toggle_auto_open_lan(),
        UiEvent::SliderChange {
            id,
            value,
            committed,
            ..
        } if id == "day_minutes" => {
            ctx.shell.set_day_minutes(value.round() as u32, committed);
        }
        UiEvent::ListSelect { id, index } if id == "mods" => {
            if let Some(session) = ctx.shell.world_settings_mut() {
                session.selected = index as usize;
            }
        }
        UiEvent::TextChanged { id, text } if id == "rename_input" => {
            ctx.ui.state_mut().set("rename_text", UiValue::Str(text));
        }
        UiEvent::Submit { id, text } if id == "rename_input" => apply_rename(ctx, &text),
        UiEvent::Click { id, .. } => match id.as_str() {
            "copy_seed" => {
                if let Some(seed) = ctx.shell.world_settings().and_then(|s| s.seed) {
                    ctx.ui.clipboard_mut().set_text(&seed.to_string());
                }
            }
            "rename" => {
                let name = ctx
                    .shell
                    .world_settings_mut()
                    .map(|s| {
                        s.renaming = true;
                        s.world_name.clone()
                    })
                    .unwrap_or_default();
                ctx.ui
                    .state_mut()
                    .set("rename_text", UiValue::Str(name.clone()));
                ctx.ui.focus_text_input("rename_input", &name, 48);
            }
            "rename_confirm" => {
                let text = ctx
                    .ui
                    .state_mut()
                    .get_str("rename_text")
                    .unwrap_or_default()
                    .to_owned();
                apply_rename(ctx, &text);
            }
            "back" => {
                ctx.shell.close_page();
                ctx.goto(AppScreen::WorldSelect);
            }
            "delete_world" => open_delete_confirm(ctx),
            _ => {}
        },
        UiEvent::Key { key, .. } => match key {
            NavKey::Escape => {
                if let Some(session) = ctx.shell.world_settings_mut() {
                    session.renaming = false;
                }
            }
            NavKey::Left | NavKey::Right => {
                if let Some(session) = ctx.shell.world_settings_mut() {
                    session.tab = match key {
                        NavKey::Left => SettingsTab::World,
                        _ => SettingsTab::Mods,
                    };
                }
            }
            NavKey::Enter => {
                if let Some((row, SettingsTab::Mods)) =
                    ctx.shell.world_settings().map(|s| (s.selected, s.tab))
                {
                    ctx.shell.toggle_world_settings_row(row);
                }
            }
            NavKey::Delete => open_delete_confirm(ctx),
            NavKey::Up => move_selection(ctx, -1),
            NavKey::Down => move_selection(ctx, 1),
            _ => {}
        },
        _ => {}
    }
}

/// Leave the settings page for the delete confirmation of its world.
fn open_delete_confirm(ctx: &mut ScreenCtx) {
    ctx.shell.close_page();
    if ctx.shell.selected_world_info().is_some() {
        ctx.goto(AppScreen::DeleteWorld);
    }
}

fn apply_rename(ctx: &mut ScreenCtx, new_name: &str) {
    let Some(session) = ctx.shell.world_settings_mut() else {
        return;
    };
    let new_name = new_name.trim();
    session.renaming = false;
    if new_name.is_empty() {
        return;
    }
    let renamed = match petramond::save::rename_world(&session.dir_name, new_name) {
        Ok(()) => {
            session.world_name = new_name.to_owned();
            true
        }
        Err(e) => {
            log::warn!("could not rename world '{}': {e}", session.world_name);
            false
        }
    };
    if renamed {
        ctx.shell.refresh_worlds();
    }
}

fn move_selection(ctx: &mut ScreenCtx, step: i32) {
    let Some(session) = ctx.shell.world_settings_mut() else {
        return;
    };
    if session.tab != SettingsTab::Mods {
        return;
    }
    mods_tab::move_selection(&mut session.selected, session.rows.len(), step);
}
