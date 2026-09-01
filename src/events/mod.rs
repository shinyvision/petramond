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
pub mod tick;

pub use crate::mob::{MobDamageFeedback, MobDamageFeedbackComponent, MobDamageSound};
#[allow(unused_imports)] // named only by tests that build a `SimCtx` by hand.
pub use bus::PostQueue;
pub use bus::{with_sessions_scope, EventBus, OpenGui, Outcome, SessionPlayerRef, SimCtx};
pub use payload::{
    AttackAttempt, BlockBreakPre, BlockPlacePre, DamageSource, DeferredAction, InteractAttempt,
    ItemUseEvent, ItemUsePre, MobDamagePre, PlayerDamagePre, PostEvent, PostEventKind,
};
pub use stages::{Attach, Stage, TickSystems};
pub use tick::ClientEvent;
