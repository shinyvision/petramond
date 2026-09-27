use mod_api::{MobSnapshot, MobTagLookup, MobTagOp, MobTagValue};

use crate::__rt::host_fn;
use crate::__rt::try_host_fn;

try_host_fn! {
    pub fn try_mob_tags_get_many(mob_ids: Vec<u64>) -> Vec<Option<Vec<(String, MobTagValue)>>>
        => MobTagsGetMany { mob_ids } => MobTagsMany
}

try_host_fn! {
    pub fn try_mob_tags_write(writes: Vec<MobTagOp>) -> Vec<bool>
        => MobTagsWrite { writes } => Bools
}

host_fn! {
    pub fn mob_tag_get(mob_id: u64, key: &str) -> MobTagLookup
        => MobTagGet { mob_id, key: key.into() } => MobTag
}

host_fn! {
    pub fn mob_tag_set(mob_id: u64, key: &str, value: MobTagValue) -> bool
        => MobTagSet { mob_id, key: key.into(), value } => Bool
}

host_fn! {
    pub fn mob_tag_delete(mob_id: u64, key: &str) -> bool
        => MobTagDelete { mob_id, key: key.into() } => Bool
}

host_fn! {
    pub fn mob_tags_get(mob_id: u64) -> Option<Vec<(String, MobTagValue)>>
        => MobTagsGet { mob_id } => MobTags
}

host_fn! {
    pub fn mobs_with_tag(key: &str, value: Option<MobTagValue>) -> Vec<MobSnapshot>
        => MobsWithTag { key: key.into(), value } => Mobs
}

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
        assert_eq!(
            <[f64; 3]>::from_tag(&point.to_tag()),
            Some(point),
            "never rounded"
        );
    }

    #[test]
    fn anything_else_reads_as_absent() {
        let text = |s: &str| MobTagValue::Str(s.into());
        assert_eq!(
            <[i32; 3]>::from_tag(&text("4 0 -2")),
            None,
            "the old decimal text"
        );
        assert_eq!(
            <[i32; 3]>::from_tag(&text("+00000040000000000000000")),
            None
        );
        assert_eq!(
            <[i32; 3]>::from_tag(&text("00000004000000000000000g")),
            None
        );
        assert_eq!(<[i32; 3]>::from_tag(&MobTagValue::I64(4)), None);
        assert_eq!(
            <[f64; 3]>::from_tag(&[1, 2, 3].to_tag()),
            None,
            "a cell is no point"
        );
    }
}

host_fn! {
    pub fn mob_tags_get_many(mob_ids: Vec<u64>) -> Vec<Option<Vec<(String, MobTagValue)>>>
        => MobTagsGetMany { mob_ids } => MobTagsMany
}

host_fn! {
    pub fn mob_tags_write(writes: Vec<MobTagOp>) -> Vec<bool>
        => MobTagsWrite { writes } => Bools
}
