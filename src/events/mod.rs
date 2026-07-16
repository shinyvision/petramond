//! Event bus + tick-stage scheduler — modding system Phase 1.
//!
//! Pure engine seams, no WASM: pre events dispatch synchronously at their decision
//! site (mutable payload, cancellable), post events queue and drain FIFO at stage
//! boundaries within the same tick, and systems attach `Before`/`After` the named
//! engine tick stages. Handler and system order is `(priority ascending,
//! registration order)` everywhere — part of the multiplayer determinism contract.
//! The engine registers nothing yet; the seams exist for mods (and future core
//! content built through the same API).

mod bus;
mod payload;
mod stages;

pub(crate) use crate::mob::{MobDamageFeedback, MobDamageFeedbackComponent, MobDamageSound};
pub(crate) use bus::{EventBus, Outcome};
#[allow(unused_imports)] // named by handler/system signatures from Phase 2 on.
pub(crate) use bus::{PostQueue, SimCtx};
pub(crate) use payload::{
    BlockBreakPre, BlockInteract, BlockPlacePre, ContainerKind, DamageSource, ItemUsePre,
    MobDamagePre, MobInteract, ModAction, PlayerDamagePre, PostEvent, PostEventKind,
};
pub(crate) use stages::{Attach, Stage, TickSystems};
