//! Options → Controls controller: a category-grouped list of every
//! remappable action — the engine's (Movement / Interacting / Other) plus
//! whatever the session's client mods registered (one category per pack).
//! Clicking a binding button arms remap mode — the App then CAPTURES the next
//! raw key/mouse/scroll as the new binding (`app/options.rs`); ESC cancels,
//! and clicking a different action's button switches the armed action.

use crate::app::App;
use petramond_ui::{UiEvent, UiMap, UiState, UiValue};
use std::sync::Arc;

/// The two strings the hint line swaps between. They share one FIXED box in
/// the document, so both have to fit it — see the test at the bottom of this
/// file, which is the only place that knows they exist.
pub(super) const HINT_ARMED: &str = "Press a key, button or wheel. ESC cancels.";
pub(super) const HINT_IDLE: &str = "Click an action to rebind it.";

/// One display row of the controls list: a category header or an action
/// (identified by its stable id in the app's action table).
pub(super) enum RowEntry {
    Header(String),
    Action(String),
}

/// The list rows in display order: table order, a header wherever the
/// category changes. Shared by `populate` (builds the bound items) and
/// `handle` (maps a clicked row index back to its action id).
pub(super) fn row_entries(table: &petramond::controls::ActionTable) -> Vec<RowEntry> {
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
    super::populate_options_chrome(app, state);
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
        HINT_ARMED
    } else {
        HINT_IDLE
    };
    state.set("remap_hint", UiValue::Str(hint.to_string()));
}

pub(super) fn handle(app: &mut App, ev: UiEvent) {
    // Back also disarms any pending remap (`close_options_category`).
    if super::options_category_back(app, &ev) {
        return;
    }
    if let UiEvent::Click { id, item, .. } = ev {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The hint box is deliberately fixed-height (a reflow between a queued
    /// press and its resolution misroutes the click), so it is the copy that
    /// has to fit — at every gui scale, including 1, where a `small` run has
    /// no smaller step to take. Longer wording needs a taller box, and only
    /// this file knows what the wording is.
    #[test]
    fn both_remap_hints_fit_the_fixed_hint_box() {
        use petramond_ui::{solve, InstTree, ThemeEnv, UiState};
        let doc = petramond::gui::documents::doc_for(petramond::gui_state::GuiKind::OptionsControls)
            .expect("controls document loads");
        let theme = petramond::gui::doc_theme::theme();
        let mut state = UiState::new();
        state.set("remap_hint", UiValue::Str(HINT_ARMED.into()));
        let tree = InstTree::expand(&doc.doc, &state);
        for scale in [1, 2, 3, 4] {
            let env = ThemeEnv {
                theme: &theme,
                gui_scale: scale,
                image_size: &|_| None,
            };
            let solved = solve(&tree, &env, (320, 240), &|_| 0);
            let hint = (0..tree.len() as u32)
                .find(|i| tree.get(*i).node.bind.text.as_deref() == Some("remap_hint"))
                .map(|i| solved.rects[i as usize])
                .expect("the hint label is in the document");
            for text in [HINT_ARMED, HINT_IDLE] {
                let (_, h) = theme.ui_font().measure(text, Some(hint.w));
                assert!(
                    h <= hint.h,
                    "@scale {scale}: {text:?} needs {h}px in a {}×{} box",
                    hint.w,
                    hint.h
                );
            }
        }
    }

    /// Every ENGINE action label is authored copy that reaches the row through
    /// a bind — so the shrink rule treats it as data and ellipsizes it, and no
    /// document guard can see it. The binding button's column is fixed, so the
    /// label column is whatever is left; a label that outgrows it reads as
    /// "Previous Ho...". Widen the panel or shorten the wording.
    #[test]
    fn every_engine_action_label_fits_the_controls_row() {
        use petramond::controls::BindableAction;
        use petramond_ui::{solve, InstTree, ThemeEnv, UiState};
        let doc = petramond::gui::documents::doc_for(petramond::gui_state::GuiKind::OptionsControls)
            .expect("controls document loads");
        let theme = petramond::gui::doc_theme::theme();
        let mut state = UiState::new();
        let rows: Vec<UiMap> = BindableAction::ALL
            .iter()
            .map(|a| {
                let mut m = UiMap::new();
                m.insert("label".into(), UiValue::Str(a.label().into()));
                m.insert("binding".into(), UiValue::Str("SCROLL DOWN".into()));
                m.insert("is_header".into(), UiValue::Bool(false));
                m.insert("is_action".into(), UiValue::Bool(true));
                m
            })
            .collect();
        state.set("rows", UiValue::List(Arc::new(rows)));
        let tree = InstTree::expand(&doc.doc, &state);
        let env = ThemeEnv {
            theme: &theme,
            gui_scale: 3,
            image_size: &|_| None,
        };
        let solved = solve(&tree, &env, (320, 240), &|_| 0);
        let mut clipped = Vec::new();
        for i in 0..tree.len() as u32 {
            let inst = tree.get(i);
            if inst.node.bind.text.as_deref() != Some("label") {
                continue;
            }
            let text = inst.text.as_deref().unwrap_or("");
            let ink = theme.ui_font().width(text);
            let box_w = solved.rects[i as usize].w;
            if ink > box_w {
                clipped.push(format!("{text:?} needs {ink}px, column is {box_w}px"));
            }
        }
        assert!(clipped.is_empty(), "clipped action labels: {clipped:#?}");
    }
}
