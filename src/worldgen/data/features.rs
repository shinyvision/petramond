//! Configured tree features (content data) — composed from reusable placers.
//!
//! Each tree is data: a trunk placer + a foliage placer + materials/height. The
//! canopies follow the canonical broadleaf-oak silhouette (see `placers/foliage`).
//! Per-biome density and the variant mix are pure data edits here.

use crate::biome::Biome;
use crate::block::Block;

use crate::worldgen::feature::placers::foliage::{
    CanopyOakFoliage, ConicalSpruceFoliage, DroopyFoliage, FlatSparseFoliage,
};
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
static SPRUCE_CONE: ConicalSpruceFoliage = ConicalSpruceFoliage;

// Tree shapes.
static OAK_SMALL_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &CANOPY,
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (5, 6),
    radius: 2,
    footprint: 3, // min trunk height 5
};
static OAK_LEAN_F: TreeFeature = TreeFeature {
    trunk: &LEANING,
    foliage: &CANOPY,
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (5, 7),
    radius: 2,
    footprint: 3,
};
static OAK_SWAMP_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &DROOPY,
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (5, 7),
    radius: 2,
    footprint: 3,
};
static OAK_SAVANNA_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &FLAT,
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (5, 7),
    radius: 3,
    footprint: 3,
};
// Big single-trunk fancy oak; limbs+crown reach ~5, footprint declared honestly.
static OAK_BIG_F: GiantOakFeature = GiantOakFeature {
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (9, 14),
    footprint: 5, // floor(9*0.618)=5 -> bare trunk >= 5 too
};

// --- Species trees: same composition, different materials + silhouette. ---
static SPRUCE_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &SPRUCE_CONE,
    log: Block::SpruceLog,
    leaf: Block::SpruceLeaves,
    height: (6, 10),
    radius: 2,
    footprint: 3,
};
static SPRUCE_TALL_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &SPRUCE_CONE,
    log: Block::SpruceLog,
    leaf: Block::SpruceLeaves,
    height: (9, 13),
    radius: 3,
    footprint: 4,
};
static BIRCH_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &CANOPY,
    log: Block::BirchLog,
    leaf: Block::BirchLeaves,
    height: (6, 8),
    radius: 2,
    footprint: 3,
};
static JUNGLE_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &CANOPY,
    log: Block::JungleLog,
    leaf: Block::JungleLeaves,
    height: (7, 11),
    radius: 3,
    footprint: 4,
};
static ACACIA_F: TreeFeature = TreeFeature {
    trunk: &LEANING,
    foliage: &FLAT,
    log: Block::AcaciaLog,
    leaf: Block::AcaciaLeaves,
    height: (5, 8),
    radius: 3,
    footprint: 4,
};
static DARK_OAK_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &CANOPY,
    log: Block::DarkOakLog,
    leaf: Block::DarkOakLeaves,
    height: (6, 8),
    radius: 3,
    footprint: 4,
};
static CHERRY_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &CANOPY,
    log: Block::CherryLog,
    leaf: Block::CherryLeaves,
    height: (6, 9),
    radius: 3,
    footprint: 4,
};

pub static OAK_SMALL: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_SMALL_F,
};
pub static OAK_LEAN: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_LEAN_F,
};
pub static OAK_SWAMP: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_SWAMP_F,
};
pub static OAK_SAVANNA: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_SAVANNA_F,
};
pub static OAK_BIG: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_BIG_F,
};
pub static SPRUCE: ConfiguredFeature = ConfiguredFeature {
    feature: &SPRUCE_F,
};
pub static SPRUCE_TALL: ConfiguredFeature = ConfiguredFeature {
    feature: &SPRUCE_TALL_F,
};
pub static BIRCH: ConfiguredFeature = ConfiguredFeature {
    feature: &BIRCH_F,
};
pub static JUNGLE: ConfiguredFeature = ConfiguredFeature {
    feature: &JUNGLE_F,
};
pub static ACACIA: ConfiguredFeature = ConfiguredFeature {
    feature: &ACACIA_F,
};
pub static DARK_OAK: ConfiguredFeature = ConfiguredFeature {
    feature: &DARK_OAK_F,
};
pub static CHERRY: ConfiguredFeature = ConfiguredFeature {
    feature: &CHERRY_F,
};

/// Per-biome tree density (probability per column). A pure data knob.
pub fn tree_density(b: Biome) -> f32 {
    match b {
        Biome::Forest => 0.055,
        Biome::BirchForest => 0.045,
        Biome::DarkForest => 0.075, // dense canopy
        Biome::Jungle => 0.070,
        Biome::Plains => 0.012,
        Biome::Meadow => 0.003,
        Biome::Savanna => 0.015,
        Biome::Foothills => 0.012,
        Biome::WindsweptHills => 0.008,
        Biome::Mountains => 0.004, // sparse, lower slopes only
        Biome::Swamp => 0.018,
        Biome::Wetland => 0.011,
        Biome::Taiga => 0.026,
        Biome::OldGrowthTaiga => 0.040,
        Biome::SnowyTaiga => 0.020,
        Biome::Grove => 0.022,
        Biome::CherryGrove => 0.030,
        Biome::SnowyTundra => 0.003,
        Biome::SnowySlopes => 0.002,
        _ => 0.0, // Ocean/DeepOcean/Beach/Desert/Badlands/River/peaks/ice/mushroom
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
        Biome::BirchForest => {
            let _ = rng.next_i32(0, 99);
            &BIRCH
        }
        Biome::Plains => match rng.next_i32(0, 99) {
            0..=9 => &OAK_BIG,
            _ => &OAK_SMALL,
        },
        Biome::Meadow => {
            let _ = rng.next_i32(0, 99);
            &OAK_BIG
        }
        Biome::Savanna => {
            let _ = rng.next_i32(0, 99);
            &ACACIA
        }
        Biome::Jungle => match rng.next_i32(0, 99) {
            0..=19 => &OAK_SMALL, // jungle bushes (small oak filler)
            _ => &JUNGLE,
        },
        Biome::DarkForest => match rng.next_i32(0, 99) {
            0..=24 => &OAK_SMALL,
            _ => &DARK_OAK,
        },
        Biome::Taiga | Biome::SnowyTaiga | Biome::Grove => {
            let _ = rng.next_i32(0, 99);
            &SPRUCE
        }
        Biome::OldGrowthTaiga => match rng.next_i32(0, 99) {
            0..=49 => &SPRUCE_TALL,
            _ => &SPRUCE,
        },
        Biome::CherryGrove => {
            let _ = rng.next_i32(0, 99);
            &CHERRY
        }
        Biome::Swamp => match rng.next_i32(0, 99) {
            0..=14 => &OAK_BIG,
            _ => &OAK_SWAMP,
        },
        Biome::Wetland => match rng.next_i32(0, 99) {
            0..=29 => &OAK_SMALL,
            _ => &OAK_SWAMP,
        },
        Biome::Foothills | Biome::Mountains | Biome::WindsweptHills | Biome::SnowySlopes => {
            let _ = rng.next_i32(0, 99);
            &OAK_SMALL
        }
        _ => match rng.next_i32(0, 99) {
            0..=2 => &OAK_BIG,
            _ => &OAK_SMALL,
        },
    }
}
