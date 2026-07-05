//! Scripted (WASM) mob-AI node dispatch — the session registry
//! `mob::behavior::wasm`'s nodes resolve through.
//!
//! Mirrors `gen::install` in spirit but stays THREAD-LOCAL: mob AI runs only
//! on the main thread (the deterministic game tick), and the registrations
//! hold `Rc` instance handles that must not cross threads. A dispatch from
//! any other thread (there are none today) simply finds no registration and
//! decides nothing.
//!
//! Dispatch is DETACHED — no simulation scope is published — because it runs
//! mid-mob-tick, where the world is immutably borrowed. Sim host calls made
//! by the guest error (decision-only contract, see `GuestCall::AiNode`); core
//! calls (RNG, log, tick) work.

use std::cell::RefCell;
use std::collections::HashMap;

use mod_api::{AiNodeCtx, AiNodeDecision, GuestCall, GuestRet};

use super::SharedInstance;

pub(super) struct AiNodeRegistration {
    pub instance: SharedInstance,
    pub callback_id: u32,
}

thread_local! {
    static INSTALLED: RefCell<HashMap<String, AiNodeRegistration>> =
        RefCell::new(HashMap::new());
}

/// Install the session's node map (empty or not — installing always is what
/// evicts a previous session's registrations). Called from
/// `ModHost::initialize`, before any mob ticks for the new session.
pub(super) fn install(map: HashMap<String, AiNodeRegistration>) {
    INSTALLED.with(|cell| *cell.borrow_mut() = map);
}

/// One node decision for one mob. `None` when the key has no live
/// registration (mod never claimed it, disabled, or mid-load) — the node
/// contributes no opinion, exactly like an engine node returning defaults.
pub(crate) fn dispatch(key: &str, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    INSTALLED.with(|cell| {
        let map = cell.borrow();
        let reg = map.get(key)?;
        let call = GuestCall::AiNode {
            callback_id: reg.callback_id,
            ctx: ctx.clone(),
        };
        let reply = reg.instance.borrow_mut().call_guest_detached(&call)?;
        match reply {
            GuestRet::AiDecision(decision) => decision,
            _ => {
                reg.instance
                    .borrow_mut()
                    .disable("returned a non-decision reply to an AI node dispatch");
                None
            }
        }
    })
}
