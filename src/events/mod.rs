//! Event bus + tick-stage scheduler — the engine seams mods attach to.
//!
//! Pure engine seams, no WASM: pre events dispatch synchronously at their decision
//! site (mutable payload, cancellable), post events queue and drain FIFO at stage
//! boundaries within the same tick, and systems attach `Before`/`After` the named
//! engine tick stages. Handler and system order is `(priority ascending,
//! registration order)` everywhere — part of the multiplayer determinism contract.
//! Engine code and WASM mods attach through the same seams; engine registrations
//! always precede mod registrations.

mod bus;
mod payload;
mod stages;

pub(crate) use crate::mob::{MobDamageFeedback, MobDamageFeedbackComponent, MobDamageSound};
#[allow(unused_imports)] // named only by tests that build a `SimCtx` by hand.
pub(crate) use bus::PostQueue;
pub(crate) use bus::{with_sessions_scope, EventBus, Outcome, SessionPlayerRef, SimCtx};
pub(crate) use payload::{
    BlockBreakPre, BlockPlacePre, DamageSource, InteractAttempt, ItemUsePre, MobDamagePre,
    ModAction, PlayerDamagePre, PostEvent, PostEventKind,
};
pub(crate) use stages::{Attach, Stage, TickSystems};
