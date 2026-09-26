//! Body conditions on live players and mobs: grants and cooling.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::EntityRef;
use crate::ids::ConditionId;
use crate::legality::prelude::*;

host_domain! {
    /// Body conditions on live players and mobs: grants and cooling.
    ConditionCall {
        /// Grant a live player or mob `ticks` of `condition` at `stage` (a stage
        /// index of the row). Fuel extends, stages only upgrade, and the damage
        /// clock of an active condition is never reset. Applying does not deal
        /// damage itself. → `Bool` (`false` = no such live body, or the body
        /// refuses the grant: its species tolerates the condition, or it touches
        /// a fluid whose contact clears it). Server only.
        EntityConditionApply {
            entity: EntityRef,
            condition: ConditionId,
            stage: u8,
            ticks: u32,
        } => legal(SERVER, Sim, Write),
        /// Consume `ticks` of a condition's time on a live body without moving its
        /// damage clock; `u32::MAX` clears it. → `Bool`. Server only.
        EntityConditionCool {
            entity: EntityRef,
            condition: ConditionId,
            ticks: u32,
        } => legal(SERVER, Sim, Write),
        /// `EntityConditionApply` / `EntityConditionCool` for many bodies in one
        /// crossing, applied in order with the single calls' rules. At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) commands.
        /// → [`HostRet::Bools`](crate::HostRet::Bools), parallel to `ops`.
        /// Server only.
        EntityConditionsMany {
            ops: Vec<crate::ConditionOp>,
        } => legal(SERVER, Sim, Write),
    }
}
