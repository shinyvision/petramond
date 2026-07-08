//! Scripted (WASM) mob-AI node dispatch — the session registry
//! `mob::behavior::wasm`'s nodes resolve through.
//!
//! Mirrors `gen::install` in spirit but stays THREAD-LOCAL: mob AI runs only
//! on the SIM thread (the deterministic game tick — the server thread since
//! multiplayer Phase D). Keeping the registry per-thread (instead of a
//! process-wide map) preserves test isolation: parallel test sessions each
//! install into their own thread. The server thread re-installs the session's
//! map on startup via [`ModHost::install_thread_ai_nodes`]. A dispatch from
//! a thread without an install simply finds no registration and decides
//! nothing.
//!
//! Dispatch is DETACHED — no simulation scope is published — because it runs
//! mid-mob-tick, where the world is immutably borrowed. Sim host calls made
//! by the guest error (decision-only contract, see `GuestCall::AiNode`); core
//! calls (RNG, log, tick) work.

use std::cell::RefCell;
use std::collections::HashMap;

use mod_api::{AiNodeCtx, AiNodeDecision, GuestCall, GuestRet};

use super::SharedInstance;

#[derive(Clone)]
pub(super) struct AiNodeRegistration {
    pub instance: SharedInstance,
    pub callback_id: u32,
}

thread_local! {
    static INSTALLED: RefCell<HashMap<String, AiNodeRegistration>> =
        RefCell::new(HashMap::new());
}

/// Install the session's node map on THIS thread (empty or not — installing
/// always is what evicts a previous session's registrations). Called from
/// `ModHost::initialize` on the constructing thread and again by the server
/// thread at startup (`ModHost::install_thread_ai_nodes`).
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
        let reply = reg.instance.lock().unwrap().call_guest_detached(&call)?;
        match reply {
            GuestRet::AiDecision(decision) => decision,
            _ => {
                reg.instance
                    .lock()
                    .unwrap()
                    .disable("returned a non-decision reply to an AI node dispatch");
                None
            }
        }
    })
}
