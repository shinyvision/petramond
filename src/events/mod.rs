//! Event bus + tick-stage scheduler, seams mods attach to.
//!
//! Pre events fire at the decision site, mutable and cancellable. Post events queue and drain FIFO
//! at stage boundaries. Systems hook Before/After named tick stages.
//!
//! Don't reorder priority-then-registration, multiplayer determinism depends on it. Engine hooks go
//! in before mods.

mod bus;
mod payload;
mod roster;
mod stages;
pub mod tick;

pub use crate::mob::{MobDamageFeedback, MobDamageFeedbackComponent, MobDamageSound};
#[allow(unused_imports)]
pub use bus::PostQueue;
pub use bus::{EventBus, Outcome, SimCtx};
pub use payload::{
    AttackAttempt, BlockBreakPre, BlockPlacePre, CellsEditPre, DamageSource, DeferredAction,
    InteractAttempt, ItemUseEvent, ItemUsePre, MobDamagePre, PlayerDamagePre, PostEvent,
    PostEventKind, ProjectileHit,
};
pub use roster::{OpenGui, PlayerRoster, RosterRefs, SessionPlayerRef};
pub use stages::{Attach, Stage, TickSystems};
pub use tick::ClientEvent;
