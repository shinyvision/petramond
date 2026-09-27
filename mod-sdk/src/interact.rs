use mod_api::ItemId;

use crate::{
    emit_sound, item_info, item_names, replace_held_one, resolve_item, PackKey, PackKeyKind,
};

pub fn block_center(pos: [i32; 3]) -> [f64; 3] {
    [
        f64::from(pos[0]) + 0.5,
        f64::from(pos[1]) + 0.5,
        f64::from(pos[2]) + 0.5,
    ]
}

pub fn cell_of(pos: [f64; 3]) -> [i32; 3] {
    [
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    ]
}

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

pub const WOODEN_BUCKET: &str = "petramond:wooden_bucket";
pub const WATER_BUCKET: &str = "petramond:water_bucket";
pub const WATER_SPLASH_SOUND: &str = "petramond:water_splash_small";

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketSwap {
    Pour,
    Scoop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaterBuckets {
    pub empty: ItemId,
    pub full: ItemId,
}

impl WaterBuckets {
    pub fn resolve() -> Option<Self> {
        Some(Self {
            empty: resolve_item(WOODEN_BUCKET)?,
            full: resolve_item(WATER_BUCKET)?,
        })
    }

    pub fn swap_for(&self, held: ItemId, vessel_full: bool) -> Option<BucketSwap> {
        match (held, vessel_full) {
            (id, false) if id == self.full => Some(BucketSwap::Pour),
            (id, true) if id == self.empty => Some(BucketSwap::Scoop),
            _ => None,
        }
    }

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
        assert_eq!(
            BUCKETS.swap_for(BUCKETS.full, false),
            Some(BucketSwap::Pour)
        );
        assert_eq!(BUCKETS.swap_for(BUCKETS.full, true), None);
    }

    #[test]
    fn an_empty_bucket_scoops_only_from_a_full_vessel() {
        assert_eq!(
            BUCKETS.swap_for(BUCKETS.empty, true),
            Some(BucketSwap::Scoop)
        );
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
