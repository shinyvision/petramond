//! Mod GUI calls: the session state map plus queued open/close requests.

use mod_api::{HostCall, HostRet};

use crate::events::ModAction;

use super::guards::{sim_call, sim_query};

/// Mod-GUI calls (session state map plus open/close).
/// State keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so unlike the persistent KV no prefix is enforced.
pub(super) fn handle_gui_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::GuiStateSet { key, value } => sim_call(|ctx| {
            crate::gui::gui_state_set(
                ctx.gui_state,
                key,
                crate::modding::convert::gui_value(value),
            )
        }),
        HostCall::GuiStateGet { key } => sim_query(|ctx| {
            HostRet::GuiValue(
                ctx.gui_state
                    .get(&key)
                    .map(crate::modding::convert::gui_value_out),
            )
        }),
        HostCall::GuiOpen { kind_key } => {
            // Resolve WITHOUT registering: opening a kind nothing declared is
            // a mod bug, reported forgivingly (like an unknown sound key).
            let Some(kind) = crate::gui::resolve_kind(&kind_key).filter(|k| k.is_mod()) else {
                log::warn!("[mod {mod_id}] GuiOpen: unknown or non-mod gui kind '{kind_key}'");
                return HostRet::Bool(false);
            };
            sim_query(|ctx| {
                ctx.queue.push_action(ModAction::OpenGui { kind });
                HostRet::Bool(true)
            })
        }
        HostCall::GuiClose => sim_call(|ctx| ctx.queue.push_action(ModAction::CloseGui)),
        other => HostRet::Error(format!(
            "non-GUI call {other:?} mis-routed to handle_gui_call (host bug)"
        )),
    }
}
