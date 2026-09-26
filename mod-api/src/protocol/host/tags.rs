//! The per-mob typed tag map.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::MobTagValue;
use crate::legality::prelude::*;

host_domain! {
    /// The per-mob typed tag map.
    TagCall {
        /// Per-mob tag map: typed key/value pairs attached to a live mob instance.
        /// → [`HostRet::MobTag`](crate::HostRet::MobTag) carrying a [`MobTagLookup`](crate::MobTagLookup):
        /// [`MissingMob`](crate::MobTagLookup::MissingMob) for a dead/absent mob,
        /// [`Absent`](crate::MobTagLookup::Absent) for a live mob not carrying the key.
        MobTagGet {
            mob_id: u64,
            key: String,
        } => legal(SERVER, Sim, Read),
        /// `false` = no such live mob, or the mob's tag map is full (32 entries)
        /// and `key` would be a NEW one — replacing an existing key always
        /// succeeds. → [`HostRet::Bool`](crate::HostRet::Bool).
        MobTagSet {
            mob_id: u64,
            key: String,
            value: MobTagValue,
        } => legal(SERVER, Sim, Write),
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (whether the key was present).
        MobTagDelete {
            mob_id: u64,
            key: String,
        } => legal(SERVER, Sim, Write),
        /// The WHOLE tag map of the live mob `mob_id`, sorted by key — one call
        /// instead of one [`TagCall::MobTagGet`] per key. `MobTags(None)` = no
        /// such live mob. → [`HostRet::MobTags`](crate::HostRet::MobTags).
        MobTagsGet {
            mob_id: u64,
        } => legal(SERVER, Sim, Read),
        /// Every live mob carrying `key` (any value); with `value: Some(v)` only
        /// those whose stored value EQUALS `v` (exact match — a `F64` NaN matches
        /// nothing). Resolved host-side against the live set, dead mobs excluded
        /// exactly like [`EntityCall::MobsInRadius`](crate::EntityCall::MobsInRadius). → [`HostRet::Mobs`](crate::HostRet::Mobs).
        MobsWithTag {
            key: String,
            value: Option<MobTagValue>,
        } => legal(SERVER, Sim, Read),
        /// [`TagCall::MobTagsGet`] for many mobs in one crossing — how a sweep
        /// reads its whole population's state. At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) ids.
        /// → [`HostRet::MobTagsMany`](crate::HostRet::MobTagsMany), parallel to
        /// `mob_ids`.
        MobTagsGetMany {
            mob_ids: Vec<u64>,
        } => legal(SERVER, Sim, Read),
        /// `MobTagSet` / `MobTagDelete` for many mobs in one crossing, applied
        /// in order with the single calls' rules (namespaced writes, the
        /// per-mob cap, the presence-transition events). At most
        /// [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) writes.
        /// → [`HostRet::Bools`](crate::HostRet::Bools), parallel to `writes`.
        MobTagsWrite {
            writes: Vec<crate::MobTagOp>,
        } => legal(SERVER, Sim, Write),
    }
}
