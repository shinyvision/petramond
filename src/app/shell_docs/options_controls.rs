//! Options → Controls controller: a category-grouped list of every
//! remappable action — the engine's (Movement / Interacting / Other) plus
//! whatever the session's client mods registered (one category per pack).
//! Clicking a binding button arms remap mode — the App then CAPTURES the next
//! raw key/mouse/scroll as the new binding (`app/options.rs`); ESC cancels,
//! and clicking a different action's button switches the armed action.

use crate::app::App;
use petramond_ui::{UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

/// One display row of the controls list: a category header or an action
/// (identified by its stable id in the app's action table).
pub(super) enum RowEntry {
    Header(String),
    Action(String),
}

/// The list rows in display order: table order, a header wherever the
/// category changes. Shared by `populate` (builds the bound items) and
/// `handle` (maps a clicked row index back to its action id).
pub(super) fn row_entries(table: &crate::controls::ActionTable) -> Vec<RowEntry> {
    let mut rows = Vec::new();
    let mut current: Option<&str> = None;
    for row in table.rows() {
        if current != Some(row.category.as_str()) {
            current = Some(row.category.as_str());
            rows.push(RowEntry::Header(row.category.to_uppercase()));
        }
        rows.push(RowEntry::Action(row.id.clone()));
    }
    rows
}

pub(super) fn populate(app: &App, state: &mut UiState) {
    state.set("show_backdrop", UiValue::Bool(app.game.is_none()));
    let remapping = app.remap.as_deref();
    let items: Vec<UiMap> = row_entries(&app.action_table)
        .into_iter()
        .map(|entry| {
            let mut m = UiMap::new();
            match entry {
                RowEntry::Header(title) => {
                    m.insert("label".into(), UiValue::Str(title));
                    m.insert("binding".into(), UiValue::Str(String::new()));
                    m.insert("is_header".into(), UiValue::Bool(true));
                    m.insert("is_action".into(), UiValue::Bool(false));
                }
                RowEntry::Action(id) => {
                    let row = app.action_table.row(&id).expect("row from table");
                    let binding = if remapping == Some(id.as_str()) {
                        "> ??? <".to_string()
                    } else {
                        app.action_table
                            .effective(&app.settings.bindings, row)
                            .label()
                    };
                    m.insert("label".into(), UiValue::Str(row.label.clone()));
                    m.insert("binding".into(), UiValue::Str(binding));
                    m.insert("is_header".into(), UiValue::Bool(false));
                    m.insert("is_action".into(), UiValue::Bool(true));
                }
            }
            m
        })
        .collect();
    state.set("rows", UiValue::List(Arc::new(items)));
    // The hint line has a FIXED slot in the document; only its text swaps, so
    // arming a remap never reflows the buttons under the cursor.
    let hint = if remapping.is_some() {
        "Press a key, button or wheel. ESC cancels."
    } else {
        "Click an action to rebind it."
    };
    state.set("remap_hint", UiValue::Str(hint.to_string()));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    if let UiEvent::Click { id, item, .. } = ev {
        if id == "back" {
            app.cancel_remap();
            app.close_options_category();
            return;
        }
        if id != "bind" {
            return;
        }
        let Some(index) = item else {
            return;
        };
        let rows = row_entries(&app.action_table);
        let Some(RowEntry::Action(action_id)) = rows.get(index as usize) else {
            return;
        };
        if app.remap.as_deref() == Some(action_id.as_str()) {
            // Clicking the armed button again disarms it.
            app.cancel_remap();
        } else {
            // Arms this action — and thereby cancels any other armed one.
            app.begin_remap(action_id);
        }
    }
}
