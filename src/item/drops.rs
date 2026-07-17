use super::ItemType;

/// One harvested drop: `min..=max` of `item`, dropped with probability `chance`.
/// A range (e.g. copper's 2–4) is rolled at spawn time; `min == max` is an exact
/// count. `chance` is the independent probability this drop appears at all (`1.0`
/// = always, e.g. ore yields); a sub-1 chance models an occasional yield such as
/// the 10% sapling a broken or decayed leaf sheds.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Drop {
    pub item: ItemType,
    pub min: u8,
    pub max: u8,
    pub chance: f32,
}

/// What a block drops when harvested (with a sufficient tool, per the mining
/// model). An empty slice = no drop.
///
/// Lives here (not in `block/`) so block defs can reference it without an
/// ownership tangle (block defs already depend on the item crate path).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DropSpec {
    pub drops: &'static [Drop],
}

impl DropSpec {
    /// No drop at all (e.g. air, water, short grass).
    pub const NONE: DropSpec = DropSpec { drops: &[] };
}
