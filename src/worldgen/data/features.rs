//! Configured features (content data) — the five oaks as composed rows.
//!
//! Each oak is now data: a trunk placer + a foliage placer + materials/params,
//! sharing reusable placers (StraightTrunk is reused by three variants;
//! BlobFoliage by two). Adding a new normal tree species is a new `TreeFeature`
//! row here reusing existing placers — no new bespoke function.
//!
//! Strata P3: `tree_density` and `pick_oak` reproduce `tree_probability` /
//! `pick_oak_variant` exactly (byte-parity). P4 replaces them with
//! `PlacedFeature` rows + a `PlacementModifier` walk per biome.

use crate::biome::Biome;
use crate::block::Block;

use crate::worldgen::feature::placers::foliage::{BlobFoliage, DroopyFoliage, OffsetBlobFoliage};
use crate::worldgen::feature::placers::trunk::{LeaningTrunk, StraightTrunk};
use crate::worldgen::feature::tree::{GiantOakFeature, TreeFeature};
use crate::worldgen::feature::ConfiguredFeature;
use crate::worldgen::rng::FeatureRng;

// Shared placer instances (zero-sized).
static STRAIGHT: StraightTrunk = StraightTrunk;
static LEANING: LeaningTrunk = LeaningTrunk;
static BLOB: BlobFoliage = BlobFoliage;
static OFFSET_BLOB: OffsetBlobFoliage = OffsetBlobFoliage;
static DROOPY: DroopyFoliage = DroopyFoliage;

// The five oak shapes.
static OAK1_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT, foliage: &BLOB,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (4, 5), radius: 2, footprint: 3,
};
static OAK2_F: TreeFeature = TreeFeature {
    trunk: &LEANING, foliage: &BLOB,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (6, 7), radius: 2, footprint: 3,
};
static OAK3_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT, foliage: &OFFSET_BLOB,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (4, 4), radius: 2, footprint: 3,
};
static OAK4_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT, foliage: &DROOPY,
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (5, 6), radius: 2, footprint: 3,
};
// oak_big branches reach ~7 from origin; footprint declared honestly (see §8).
static OAK_BIG_F: GiantOakFeature = GiantOakFeature {
    log: Block::OakLog, leaf: Block::OakLeaves,
    height: (8, 12), footprint: 7,
};

pub static OAK1: ConfiguredFeature = ConfiguredFeature { feature: &OAK1_F };
pub static OAK2: ConfiguredFeature = ConfiguredFeature { feature: &OAK2_F };
pub static OAK3: ConfiguredFeature = ConfiguredFeature { feature: &OAK3_F };
pub static OAK4: ConfiguredFeature = ConfiguredFeature { feature: &OAK4_F };
pub static OAK_BIG: ConfiguredFeature = ConfiguredFeature { feature: &OAK_BIG_F };

/// Per-biome tree density. P4 modestly enriches the wooded biomes (a pure data
/// edit — the only knob that controls forest fullness). Combined with the P4
/// cross-chunk placement (no more bald chunk-edge seams), forests read as full,
/// continuous canopies rather than sparse grids. Tune freely here.
pub fn tree_density(b: Biome) -> f32 {
    match b {
        Biome::Forest => 0.09,
        Biome::BirchForest => 0.06,
        Biome::Plains => 0.018,
        Biome::Savanna => 0.018,
        Biome::Swamp => 0.020,
        Biome::Taiga => 0.018,
        Biome::SnowyTaiga => 0.016,
        Biome::SnowyTundra => 0.003,
        _ => 0.0,
    }
}

/// Pick an oak variant for a biome (verbatim `pick_oak_variant`), as a
/// configured feature. Draws exactly one `next_i32(0,99)`.
pub fn pick_oak(rng: &mut FeatureRng, b: Biome) -> &'static ConfiguredFeature {
    match b {
        Biome::Forest => match rng.next_i32(0, 99) {
            0..=4 => &OAK_BIG,
            5..=44 => &OAK2,
            45..=74 => &OAK3,
            _ => &OAK1,
        },
        Biome::Plains | Biome::Savanna => match rng.next_i32(0, 99) {
            0..=2 => &OAK_BIG,
            3..=72 => &OAK1,
            _ => &OAK4,
        },
        Biome::Swamp => match rng.next_i32(0, 99) {
            0..=9 => &OAK_BIG,
            _ => &OAK4,
        },
        _ => match rng.next_i32(0, 99) {
            0..=2 => &OAK_BIG,
            _ => &OAK1,
        },
    }
}
