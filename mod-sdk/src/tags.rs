//! Per-mob tags: typed key/value pairs attached to a live mob instance — THE
//! per-mob keyed store (there is no separate per-mob byte KV).
//!
//! Tags are TYPED ([`MobTagValue`]), persist with the mob's save record, and
//! are visible to the engine's AI (a mod can steer herd behavior by tagging
//! mobs). A species' `mobs.json` row seeds spawn tags (the engine's own
//! `petramond:health` rides there). Keys are namespaced exactly like KV:
//! writes need this mod's own prefix or an engine-exposed `petramond:*` key;
//! reads may cross namespaces. A mob carries at most 32 tags; replacing an
//! existing key never counts against the cap.

use mod_api::{MobSnapshot, MobTagLookup, MobTagValue};

use crate::__rt::host_fn;

host_fn! {
    /// Read one tag on a live mob (STABLE mob id). The [`MobTagLookup`]
    /// outcome tells a GONE mob (dead/unloaded — give up) apart from a live
    /// mob simply not carrying the key.
    pub fn mob_tag_get(mob_id: u64, key: &str) -> MobTagLookup
        => MobTagGet { mob_id, key: key.into() } => MobTag
}

host_fn! {
    /// Write a tag on a live mob (own-namespace or exposed `petramond:*` key
    /// required); persists with the mob's save record. `false` = no such live
    /// mob, or the mob already carries 32 tags and `key` would be a NEW one.
    pub fn mob_tag_set(mob_id: u64, key: &str, value: MobTagValue) -> bool
        => MobTagSet { mob_id, key: key.into(), value } => Bool
}

host_fn! {
    /// Delete a tag from a live mob (own-namespace key required); `false` =
    /// the key (or the mob) was absent.
    pub fn mob_tag_delete(mob_id: u64, key: &str) -> bool
        => MobTagDelete { mob_id, key: key.into() } => Bool
}

host_fn! {
    /// Read a live mob's WHOLE tag map, sorted by key — one call instead of
    /// one [`mob_tag_get`] per key. `None` = no such live mob.
    pub fn mob_tags_get(mob_id: u64) -> Option<Vec<(String, MobTagValue)>>
        => MobTagsGet { mob_id } => MobTags
}

host_fn! {
    /// Snapshot every live mob carrying `key` (any value); with `value:
    /// Some(v)` only those whose stored value EQUALS `v` (exact match — a
    /// `F64` NaN matches nothing). Resolved host-side; dead mobs excluded,
    /// exactly like [`mobs_in_radius`](crate::mobs_in_radius).
    pub fn mobs_with_tag(key: &str, value: Option<MobTagValue>) -> Vec<MobSnapshot>
        => MobsWithTag { key: key.into(), value } => Mobs
}
