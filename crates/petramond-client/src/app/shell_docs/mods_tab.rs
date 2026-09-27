use crate::app::shell_state::ModPackRow;
use petramond::save::settings::WorldSettings;
use petramond_ui::{UiMap, UiState, UiValue};
use std::sync::Arc;

pub(super) fn populate(
    rows: &[ModPackRow],
    settings: &WorldSettings,
    selected: usize,
    state: &mut UiState,
) {
    let bound: Vec<UiMap> = rows
        .iter()
        .zip(petramond_world::assets::packs())
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
            let icon = crate::app::content::pack_icon_name(
                &asset
                    .id
                    .clone()
                    .unwrap_or_else(|| crate::app::content::dir_name(&asset.dir)),
            );
            let has_icon = crate::app::content::pack_icons()
                .iter()
                .any(|i| i.key == icon);
            m.insert("has_icon".into(), UiValue::Bool(has_icon));
            m.insert("icon".into(), UiValue::Str(icon));
            m.insert(
                "is_addon".into(),
                UiValue::Bool(crate::app::content::is_addon(asset)),
            );
            m
        })
        .collect();
    state.set("no_mods", UiValue::Bool(bound.is_empty()));
    state.set("mod_rows", UiValue::List(Arc::new(bound)));
    state.set("mod_sel", UiValue::I32(selected as i32));
}

pub(super) fn populate_tabs(tab: crate::app::shell_state::SettingsTab, state: &mut UiState) {
    use crate::app::shell_state::SettingsTab;
    state.set("tab_sel", UiValue::I32(tab.index()));
    state.set("tab_world", UiValue::Bool(tab == SettingsTab::World));
    state.set("tab_mods", UiValue::Bool(tab == SettingsTab::Mods));
}

pub(super) fn move_selection(selected: &mut usize, rows: usize, step: i32) {
    if rows == 0 {
        return;
    }
    *selected = (*selected as i32 + step).clamp(0, rows as i32 - 1) as usize;
}
