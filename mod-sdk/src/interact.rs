//! Small interaction idioms many mods share: cell/point conversions, the
//! sneak-to-build gate, and the water-bucket swap every fluid vessel (trough,
//! cauldron, any future barrel or kettle) performs. The engine row names the
//! bucket swap needs are declared here once, so a vessel mod never spells
//! them itself.

use mod_api::ItemId;

use crate::{
    emit_sound, item_info, item_names, replace_held_one, resolve_item, PackKey, PackKeyKind,
};

/// The centre of block cell `pos` — where a cell's sounds and particles
/// originate.
pub fn block_center(pos: [i32; 3]) -> [f64; 3] {
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ]
}

/// The block cell containing world point `pos` (floor on every axis).
pub fn cell_of(pos: [f64; 3]) -> [i32; 3] {
    [
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ]
}

/// Whether the held item places a block (its row carries a `block` link) —
/// the gate a sneak-to-build rule reads before claiming a click. Registry
/// only, legal on any instance; no item, or an unresolvable id, reads as
/// "not a block".
pub fn held_item_places_block(held: Option<ItemId>) -> bool {
    let Some(id) = held else {
        return false;
    };
    item_names(vec![id])
        .into_iter()
        .next()
        .flatten()
        .and_then(|name| item_info(&name))
        .is_some_and(|info| info.block.is_some())
}

/// The engine's empty bucket row.
pub const WOODEN_BUCKET: &str = "petramond:wooden_bucket";
/// The engine's full water bucket row.
pub const WATER_BUCKET: &str = "petramond:water_bucket";
/// The engine sound a bucket pour or scoop plays.
pub const WATER_SPLASH_SOUND: &str = "petramond:water_splash_small";

/// The engine ids [`WaterBuckets`] depends on, for a vessel mod's pack
/// validation test (`pack_check::assert_declared`).
pub const WATER_BUCKET_KEYS: &[PackKey] = &[
    PackKey {
        kind: PackKeyKind::Item,
        key: WOODEN_BUCKET,
    },
    PackKey {
        kind: PackKeyKind::Item,
        key: WATER_BUCKET,
    },
    PackKey {
        kind: PackKeyKind::Sound,
        key: WATER_SPLASH_SOUND,
    },
];

/// Which way a bucket exchanges water with a vessel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketSwap {
    /// A water bucket empties into the vessel and becomes an empty bucket.
    Pour,
    /// An empty bucket fills from the vessel and becomes a water bucket.
    Scoop,
}

/// The engine's resolved bucket pair: the fluid-vessel interaction helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaterBuckets {
    /// The empty bucket ([`WOODEN_BUCKET`]).
    pub empty: ItemId,
    /// The water bucket ([`WATER_BUCKET`]).
    pub full: ItemId,
}

impl WaterBuckets {
    /// Resolve both buckets. `None` when the base content is missing — the
    /// vessel then offers no bucket interaction.
    pub fn resolve() -> Option<Self> {
        Some(Self {
            empty: resolve_item(WOODEN_BUCKET)?,
            full: resolve_item(WATER_BUCKET)?,
        })
    }

    /// The swap `held` offers against a vessel that `vessel_full` says holds
    /// water or not: a water bucket pours into an empty vessel, an empty
    /// bucket scoops from a full one. Pure, so client prediction and the
    /// authoritative consumer classify identically.
    pub fn swap_for(&self, held: ItemId, vessel_full: bool) -> Option<BucketSwap> {
        match (held, vessel_full) {
            (id, false) if id == self.full => Some(BucketSwap::Pour),
            (id, true) if id == self.empty => Some(BucketSwap::Scoop),
            _ => None,
        }
    }

    /// Perform `swap` at vessel cell `pos`: exchange the held bucket for its
    /// counterpart, run `flip` (the caller's block/KV change to the vessel),
    /// then play the splash. `false` — nothing changed, `flip` not run — when
    /// the hand no longer holds the bucket the swap spends or the exchange has
    /// no room.
    pub fn perform(&self, swap: BucketSwap, pos: [i32; 3], flip: impl FnOnce()) -> bool {
        let (spent, replacement) = match swap {
            BucketSwap::Pour => (self.full, WOODEN_BUCKET),
            BucketSwap::Scoop => (self.empty, WATER_BUCKET),
        };
        if !replace_held_one(spent, replacement) {
            return false;
        }
        flip();
        emit_sound(WATER_SPLASH_SOUND, Some(block_center(pos)));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUCKETS: WaterBuckets = WaterBuckets {
        empty: ItemId(7),
        full: ItemId(8),
    };

    #[test]
    fn a_cell_centre_and_the_cell_of_a_point_round_trip() {
        assert_eq!(block_center([1, -2, 0]), [1.5, -1.5, 0.5]);
        assert_eq!(cell_of([1.5, -1.5, 0.5]), [1, -2, 0]);
        assert_eq!(cell_of([-0.01, 0.0, 15.99]), [-1, 0, 15]);
    }

    #[test]
    fn a_water_bucket_pours_only_into_an_empty_vessel() {
        assert_eq!(BUCKETS.swap_for(BUCKETS.full, false), Some(BucketSwap::Pour));
        assert_eq!(BUCKETS.swap_for(BUCKETS.full, true), None);
    }

    #[test]
    fn an_empty_bucket_scoops_only_from_a_full_vessel() {
        assert_eq!(BUCKETS.swap_for(BUCKETS.empty, true), Some(BucketSwap::Scoop));
        assert_eq!(BUCKETS.swap_for(BUCKETS.empty, false), None);
    }

    #[test]
    fn any_other_item_offers_no_swap() {
        assert_eq!(BUCKETS.swap_for(ItemId(9), false), None);
        assert_eq!(BUCKETS.swap_for(ItemId(9), true), None);
    }

    #[test]
    fn no_held_item_places_no_block() {
        assert!(!held_item_places_block(None));
    }
}
