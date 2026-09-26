//! Missing Mods controller: the mod ids a refused join reported this client
//! lacks. Back (and Enter) returns to the connect screen with the attempted
//! address intact; ESC does the same via the global close-screen control.

use super::{ScreenCtx, ShellCommand};
use petramond_ui::{NavKey, UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    let rows: Vec<UiMap> = ctx
        .shell
        .connect
        .missing
        .iter()
        .map(|m| {
            let mut row = UiMap::new();
            row.insert("id".into(), UiValue::Str(m.id.clone()));
            row.insert("has_version".into(), UiValue::Bool(!m.version.is_empty()));
            let version = if m.version.is_empty() {
                String::new()
            } else {
                format!("v{}", m.version)
            };
            row.insert("version".into(), UiValue::Str(version));
            row
        })
        .collect();
    state.set("missing_rows", UiValue::List(Arc::new(rows)));
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    match ev {
        UiEvent::Click { id, .. } if id == "back" => ctx.request(ShellCommand::ReopenConnectServer),
        UiEvent::Key {
            key: NavKey::Enter, ..
        } => ctx.request(ShellCommand::ReopenConnectServer),
        _ => {}
    }
}
