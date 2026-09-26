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

/// A value with no [`MobTagValue`] variant of its own, carried in a tag
/// through ONE shared, lossless encoding — so a tick system and the AI node
/// it steers never hand-roll text coordinates, never round them, and decode
/// without allocating. A value that does not decode reads as absent.
///
/// Cells and points ride in [`MobTagValue::Str`] as fixed-width lowercase
/// hex of their exact bits (8 digits per `i32`, 16 per `f64`).
pub trait TagValue: Sized {
    fn to_tag(&self) -> MobTagValue;
    fn from_tag(tag: &MobTagValue) -> Option<Self>;
}

impl TagValue for [i32; 3] {
    fn to_tag(&self) -> MobTagValue {
        let [x, y, z] = self.map(|v| v as u32);
        MobTagValue::Str(format!("{x:08x}{y:08x}{z:08x}"))
    }

    fn from_tag(tag: &MobTagValue) -> Option<Self> {
        let words = hex_words(tag, 8)?;
        Some(words.map(|w| w as u32 as i32))
    }
}

impl TagValue for [f64; 3] {
    fn to_tag(&self) -> MobTagValue {
        let [x, y, z] = self.map(f64::to_bits);
        MobTagValue::Str(format!("{x:016x}{y:016x}{z:016x}"))
    }

    fn from_tag(tag: &MobTagValue) -> Option<Self> {
        let words = hex_words(tag, 16)?;
        Some(words.map(f64::from_bits))
    }
}

/// Three fixed-width hex words, `width` (at most 16) digits each, filling
/// the whole tag.
fn hex_words(tag: &MobTagValue, width: usize) -> Option<[u64; 3]> {
    let MobTagValue::Str(text) = tag else {
        return None;
    };
    if text.len() != 3 * width || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let word = |i: usize| u64::from_str_radix(&text[i * width..(i + 1) * width], 16).ok();
    Some([word(0)?, word(1)?, word(2)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_and_points_round_trip_bit_exact() {
        for cell in [[0, 0, 0], [4, -2, 17], [i32::MIN, i32::MAX, -1]] {
            assert_eq!(<[i32; 3]>::from_tag(&cell.to_tag()), Some(cell));
        }
        let point = [0.1 + 0.2, -64.000_000_1, 1.0e300];
        assert_eq!(<[f64; 3]>::from_tag(&point.to_tag()), Some(point), "never rounded");
    }

    #[test]
    fn anything_else_reads_as_absent() {
        let text = |s: &str| MobTagValue::Str(s.into());
        assert_eq!(<[i32; 3]>::from_tag(&text("4 0 -2")), None, "the old decimal text");
        assert_eq!(<[i32; 3]>::from_tag(&text("+00000040000000000000000")), None);
        assert_eq!(<[i32; 3]>::from_tag(&text("00000004000000000000000g")), None);
        assert_eq!(<[i32; 3]>::from_tag(&MobTagValue::I64(4)), None);
        assert_eq!(<[f64; 3]>::from_tag(&[1, 2, 3].to_tag()), None, "a cell is no point");
    }
}
