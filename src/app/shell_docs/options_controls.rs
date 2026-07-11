//! Options → Controls controller: one row per remappable action. Clicking a
//! binding button arms remap mode — the App then CAPTURES the next raw
//! key/mouse/scroll as the new binding (`app/options.rs`); ESC cancels, and
//! clicking a different action's button switches the armed action.

use crate::app::App;
use crate::controls::BindableAction;
use petramond_ui::{UiEvent, UiState, UiValue};

pub(super) fn populate(app: &App, state: &mut UiState) {
    state.set("show_backdrop", UiValue::Bool(app.game.is_none()));
    let remapping = app.remap;
    for action in BindableAction::ALL {
        let text = if remapping == Some(action) {
            "> ??? <".to_string()
        } else {
            app.settings.bindings.binding(action).label()
        };
        state.set(format!("bind:{}", action.id()), UiValue::Str(text));
    }
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
    if let UiEvent::Click { id, .. } = ev {
        if id == "back" {
            app.cancel_remap();
            app.close_options_category();
            return;
        }
        if let Some(action) = id.strip_prefix("bind:").and_then(BindableAction::from_id) {
            if app.remap == Some(action) {
                // Clicking the armed button again disarms it.
                app.cancel_remap();
            } else {
                // Arms this action — and thereby cancels any other armed one.
                app.begin_remap(action);
            }
        }
    }
}
