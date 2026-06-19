//! Configured tree features (content data) — composed from reusable placers.
//!
//! Each tree is data: a trunk placer + a foliage placer + materials/height. The
//! canopies follow the canonical Minecraft oak silhouette (see `placers/foliage`).
//! Per-biome density and the variant mix are pure data edits here.

use crate::biome::Biome;
use crate::block::Block;

use crate::worldgen::feature::placers::foliage::{CanopyOakFoliage, DroopyFoliage, FlatSparseFoliage};
use crate::worldgen::feature::placers::trunk::{LeaningTrunk, StraightTrunk};
use crate::worldgen::feature::tree::{GiantOakFeature, TreeFeature};
use crate::worldgen::feature::ConfiguredFeature;
use crate::worldgen::rng::FeatureRng;

// Shared placer instances (zero-sized).
static STRAIGHT: StraightTrunk = StraightTrunk;
static LEANING: LeaningTrunk = LeaningTrunk;
static CANOPY: CanopyOakFoliage = CanopyOakFoliage;
static DROOPY: DroopyFoliage = DroopyFoliage;
static FLAT: FlatSparseFoliage = FlatSparseFoliage;

// Tree shapes.
static OAK_SMALL_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT, foliage: &CANOPY,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (5, 6), radius: 2, footprint: 3, // min trunk height 5
};
static OAK_LEAN_F: TreeFeature = TreeFeature {
    trunk: &LEANING, foliage: &CANOPY,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (5, 7), radius: 2, footprint: 3,
};
static OAK_SWAMP_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT, foliage: &DROOPY,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (5, 7), radius: 2, footprint: 3,
};
static OAK_SAVANNA_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT, foliage: &FLAT,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (5, 7), radius: 3, footprint: 3,
};
// Big single-trunk fancy oak; limbs+crown reach ~5, footprint declared honestly.
static OAK_BIG_F: GiantOakFeature = GiantOakFeature {
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (9, 14), footprint: 5, // floor(9*0.618)=5 -> bare trunk >= 5 too
};

pub static OAK_SMALL: ConfiguredFeature = ConfiguredFeature { feature: &OAK_SMALL_F };
pub static OAK_LEAN: ConfiguredFeature = ConfiguredFeature { feature: &OAK_LEAN_F };
pub static OAK_SWAMP: ConfiguredFeature = ConfiguredFeature { feature: &OAK_SWAMP_F };
pub static OAK_SAVANNA: ConfiguredFeature = ConfiguredFeature { feature: &OAK_SAVANNA_F };
pub static OAK_BIG: ConfiguredFeature = ConfiguredFeature { feature: &OAK_BIG_F };

/// Per-biome tree density (probability per column). A pure data knob.
pub fn tree_density(b: Biome) -> f32 {
    match b {
        Biome::Forest => 0.10,
        Biome::BirchForest => 0.06,
        Biome::Plains => 0.012,
        Biome::Savanna => 0.015,
        Biome::Foothills => 0.012,
        Biome::Mountains => 0.004, // sparse, lower slopes only
        Biome::Swamp => 0.022,
        Biome::Wetland => 0.014,
        Biome::Taiga => 0.018,
        Biome::SnowyTaiga => 0.014,
        Biome::SnowyTundra => 0.003,
        _ => 0.0, // Ocean/DeepOcean/Beach/Desert/River/SnowyPeaks
    }
}

/// Pick a tree variant for a biome. Every arm draws EXACTLY ONE `next_i32(0,99)`
/// so the RNG stream offset is biome-independent (seam replay stays deterministic).
pub fn pick_oak(rng: &mut FeatureRng, b: Biome) -> &'static ConfiguredFeature {
    match b {
        Biome::Forest => match rng.next_i32(0, 99) {
            0..=4 => &OAK_BIG,
            5..=29 => &OAK_LEAN,
            _ => &OAK_SMALL,
        },
        Biome::Plains => match rng.next_i32(0, 99) {
            0..=9 => &OAK_BIG,
            _ => &OAK_SMALL,
        },
        Biome::Savanna => {
            let _ = rng.next_i32(0, 99);
            &OAK_SAVANNA
        }
        Biome::Swamp => match rng.next_i32(0, 99) {
            0..=14 => &OAK_BIG,
            _ => &OAK_SWAMP,
        },
        Biome::Wetland => match rng.next_i32(0, 99) {
            0..=29 => &OAK_SMALL,
            _ => &OAK_SWAMP,
        },
        Biome::Foothills | Biome::Mountains => {
            let _ = rng.next_i32(0, 99);
            &OAK_SMALL
        }
        _ => match rng.next_i32(0, 99) {
            0..=2 => &OAK_BIG,
            _ => &OAK_SMALL,
        },
    }
}
