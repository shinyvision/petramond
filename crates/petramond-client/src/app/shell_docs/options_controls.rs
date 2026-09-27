use super::ScreenCtx;
use petramond_ui::{UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

pub(super) const HINT_ARMED: &str = "Press a key, button or wheel. ESC cancels.";
pub(super) const HINT_IDLE: &str = "Click an action to rebind it.";

pub(super) enum RowEntry {
    Header(String),
    Action(String),
}

pub(super) fn row_entries(table: &petramond_input::controls::ActionTable) -> Vec<RowEntry> {
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

pub(super) fn populate(ctx: &ScreenCtx, state: &mut UiState) {
    super::populate_options_chrome(ctx, state);
    let remapping = ctx.options.remap();
    let items: Vec<UiMap> = row_entries(ctx.action_table)
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
                    let row = ctx.action_table.row(&id).expect("row from table");
                    let binding = if remapping == Some(id.as_str()) {
                        "> ??? <".to_string()
                    } else {
                        ctx.action_table
                            .effective(&ctx.options.settings.bindings, row)
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
    let hovered = ctx
        .ui
        .hover_item("controls")
        .and_then(|index| items.get(index));
    let tip = hovered.filter(|row| row.get("is_action") == Some(&UiValue::Bool(true)));
    state.set("show_binding_tip", UiValue::Bool(tip.is_some()));
    for (key, field) in [
        ("binding_tip_action", "label"),
        ("binding_tip_keys", "binding"),
    ] {
        state.set(
            key,
            tip.and_then(|row| row.get(field))
                .cloned()
                .unwrap_or_else(|| UiValue::Str(String::new())),
        );
    }
    state.set("rows", UiValue::List(Arc::new(items)));
    let hint = if remapping.is_some() {
        HINT_ARMED
    } else {
        HINT_IDLE
    };
    state.set("remap_hint", UiValue::Str(hint.to_string()));
}

pub(super) fn handle(ctx: &mut ScreenCtx, ev: UiEvent) {
    if super::options_category_back(ctx, &ev) {
        return;
    }
    if let UiEvent::Click { id, item, .. } = ev {
        if id != "bind" {
            return;
        }
        let Some(index) = item else {
            return;
        };
        let rows = row_entries(ctx.action_table);
        let Some(RowEntry::Action(action_id)) = rows.get(index as usize) else {
            return;
        };
        if ctx.options.remap() == Some(action_id.as_str()) {
            ctx.options.cancel_remap();
        } else {
            ctx.options.begin_remap(action_id);
        }
    }
}

#[cfg(test)]
mod tests;
