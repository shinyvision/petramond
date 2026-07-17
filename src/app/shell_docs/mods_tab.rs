//! Shared Mods-tab behavior for the tabbed World Settings and Create World
//! screens: pack-row binding against a `WorldSettings` disabled set, and the
//! pack-icon extra images both documents' list rows reference.

use crate::app::shell::ModPackRow;
use crate::save::settings::WorldSettings;
use petramond_ui::{UiMap, UiState, UiValue};
use std::path::PathBuf;
use std::sync::Arc;

/// Per-row pack icons for the documents' `bind.image` — registered as extra
/// images on the UI driver before `populate` runs.
pub(super) fn extra_images() -> Vec<(String, PathBuf)> {
    crate::assets::packs()
        .iter()
        .filter_map(|pack| {
            let icon = pack.icon.clone()?;
            Some((icon_name(pack.id.as_deref(), &pack.name), icon))
        })
        .collect()
}

fn icon_name(id: Option<&str>, name: &str) -> String {
    format!("pack_icon:{}", id.unwrap_or(name))
}

/// Bind the Mods-tab rows: one entry per installed pack, enabled state read
/// from `settings.disabled_mods`. `rows` is parallel to
/// `crate::assets::packs()` (both screens build it from pack discovery).
pub(super) fn populate(
    rows: &[ModPackRow],
    settings: &WorldSettings,
    selected: usize,
    state: &mut UiState,
) {
    let bound: Vec<UiMap> = rows
        .iter()
        .zip(crate::assets::packs())
        .map(|(pack, asset)| {
            let mut m = UiMap::new();
            m.insert("name".into(), UiValue::Str(pack.name.clone()));
            let version = pack.version.as_ref().map(|v| format!("v{v}"));
            m.insert("has_version".into(), UiValue::Bool(version.is_some()));
            m.insert("version".into(), UiValue::Str(version.unwrap_or_default()));
            let desc = pack
                .summary
                .clone()
                .unwrap_or_else(|| pack.description.clone());
            m.insert("desc".into(), UiValue::Str(desc));
            let toggleable = pack.id.is_some();
            let enabled = match &pack.id {
                Some(id) => !settings.disabled_mods.contains(id),
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
    state.set("no_mods", UiValue::Bool(bound.is_empty()));
    state.set("mod_rows", UiValue::List(Arc::new(bound)));
    state.set("mod_sel", UiValue::I32(selected as i32));
}

/// Bind the shared tab-bar state (`tab_sel` + the two page visibility keys).
pub(super) fn populate_tabs(tab: crate::app::shell::SettingsTab, state: &mut UiState) {
    use crate::app::shell::SettingsTab;
    state.set("tab_sel", UiValue::I32(tab.index()));
    state.set("tab_world", UiValue::Bool(tab == SettingsTab::World));
    state.set("tab_mods", UiValue::Bool(tab == SettingsTab::Mods));
}

/// Clamp-move a Mods-list keyboard selection by `step`.
pub(super) fn move_selection(selected: &mut usize, rows: usize, step: i32) {
    if rows == 0 {
        return;
    }
    *selected = (*selected as i32 + step).clamp(0, rows as i32 - 1) as usize;
}
