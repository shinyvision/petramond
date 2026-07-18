//! Scripted (WASM) mob-AI node dispatch — the session registry
//! `mob::behavior::wasm`'s nodes resolve through.
//!
//! Mirrors `gen::install` in spirit but stays THREAD-LOCAL: mob AI runs only
//! on the SIM thread (the deterministic game tick — the server
//! thread). Keeping the registry per-thread (instead of a
//! process-wide map) preserves test isolation: parallel test sessions each
//! install into their own thread. The server thread re-installs the session's
//! map on startup via [`ModHost::install_thread_ai_nodes`]. A dispatch from
//! a thread without an install simply finds no registration and decides
//! nothing.
//!
//! Dispatch is DETACHED — no simulation scope is published — because it runs
//! mid-mob-tick, where the world is immutably borrowed. Sim host calls made
//! by the guest error (decision-only contract, see `GuestCall::AiNode`); the
//! core calls work, `CurrentTick` included: the dispatcher publishes the
//! tick it snapshotted into `AiNodeCtx` ([`detached_tick`]) so the tick
//! clock never needs the sim scope here.

use std::cell::{Cell, RefCell};
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
    /// The game tick of the in-flight detached AI dispatch — what
    /// `HostCall::CurrentTick` reads when no sim scope is active.
    static DETACHED_TICK: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Install the session's node map on THIS thread (empty or not — installing
/// always is what evicts a previous session's registrations). Called from
/// `ModHost::initialize` on the constructing thread and again by the server
/// thread at startup (`ModHost::install_thread_ai_nodes`).
pub(super) fn install(map: HashMap<String, AiNodeRegistration>) {
    INSTALLED.with(|cell| *cell.borrow_mut() = map);
}

/// Whether `key` has a live registration on this thread — the pre-dispatch
/// gate that lets an unclaimed scripted node (mod disabled, mid-load) skip
/// building its ctx snapshot entirely.
pub(crate) fn is_claimed(key: &str) -> bool {
    INSTALLED.with(|cell| cell.borrow().contains_key(key))
}

/// The tick published for the current detached AI dispatch, if one is in
/// flight on this thread. Read by the `CurrentTick` host-call handler as its
/// scope-free fallback.
pub(crate) fn detached_tick() -> Option<u64> {
    DETACHED_TICK.with(Cell::get)
}

/// Publish `tick` as this thread's detached-dispatch tick for the duration of
/// `f` — wrapped around every guest AI call by [`dispatch`].
pub(crate) fn with_detached_tick<T>(tick: u64, f: impl FnOnce() -> T) -> T {
    DETACHED_TICK.with(|t| t.set(Some(tick)));
    let out = f();
    DETACHED_TICK.with(|t| t.set(None));
    out
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
        let reply =
            with_detached_tick(ctx.tick, || reg.instance.lock().unwrap().call_guest_detached(&call));
        match reply? {
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
