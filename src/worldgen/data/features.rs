//! Configured tree features (content data) — composed from reusable placers.
//!
//! Each tree is data: a trunk placer + a foliage placer + materials/height. The
//! canopies follow the canonical broadleaf-oak silhouette (see `placers/foliage`).
//! Biome modules decide which configured feature to place.

use crate::block::Block;

use crate::worldgen::feature::placers::foliage::{
    BlobFoliage, ConiferFoliage, DroopyFoliage, FlatSparseFoliage,
};
use crate::worldgen::feature::placers::trunk::{LeaningTrunk, StraightTrunk};
use crate::worldgen::feature::tree::{GiantOakFeature, RedwoodFeature, TreeFeature};
use crate::worldgen::feature::ConfiguredFeature;
use crate::worldgen::rng::FeatureRng;

// Shared trunk placers (zero-sized strategies; height is per-tree config).
static STRAIGHT: StraightTrunk = StraightTrunk;
static LEANING: LeaningTrunk = LeaningTrunk;

// Foliage configs — each carries its own shape params. Trees name one; identical
// configs are shared, divergence is just a new literal here (no new impl).
static BLOB_SMALL: BlobFoliage = BlobFoliage {
    base_radius: 2,
    top_radius: 1,
    corner_cut: 0.5,
};
static BLOB_LARGE: BlobFoliage = BlobFoliage {
    base_radius: 3,
    top_radius: 2,
    corner_cut: 0.5,
};
static DROOPY: DroopyFoliage = DroopyFoliage {
    radius: 2,
    ragged: 0.15,
    drip_skip: 0.45,
};
static FLAT: FlatSparseFoliage = FlatSparseFoliage {
    upper_radius: 3,
    upper_skip: 0.30,
    lower_radius: 2,
    lower_skip: 0.40,
};
static CONIFER_SMALL: ConiferFoliage = ConiferFoliage {
    radius: 2,
    skirt_ragged: 0.25,
};

// Tree shapes.
static OAK_SMALL_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &BLOB_SMALL,
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (5, 6),
};
static OAK_SWAMP_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &DROOPY,
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (5, 7),
};
// Big single-trunk fancy oak; limbs+crown reach ~5.
static OAK_BIG_F: GiantOakFeature = GiantOakFeature {
    log: Block::OakLog,
    leaf: Block::OakLeaves,
    height: (9, 14),
};
// Huge redwood; tall trunk + long spreading limbs + wide crown, built from the
// dedicated redwood log/leaf blocks.
static REDWOOD_F: RedwoodFeature = RedwoodFeature {
    log: Block::RedwoodLog,
    leaf: Block::RedwoodLeaves,
    height: (38, 52),
};

// --- Species trees: same composition, different materials + silhouette. ---
static SPRUCE_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &CONIFER_SMALL,
    log: Block::SpruceLog,
    leaf: Block::SpruceLeaves,
    height: (6, 10),
};
static BIRCH_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &BLOB_SMALL,
    log: Block::BirchLog,
    leaf: Block::BirchLeaves,
    height: (6, 8),
};
static JUNGLE_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &BLOB_LARGE,
    log: Block::JungleLog,
    leaf: Block::JungleLeaves,
    height: (7, 11),
};
static ACACIA_F: TreeFeature = TreeFeature {
    trunk: &LEANING,
    foliage: &FLAT,
    log: Block::AcaciaLog,
    leaf: Block::AcaciaLeaves,
    height: (5, 8),
};
static DARK_OAK_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &BLOB_LARGE,
    log: Block::DarkOakLog,
    leaf: Block::DarkOakLeaves,
    height: (6, 8),
};
static CHERRY_F: TreeFeature = TreeFeature {
    trunk: &STRAIGHT,
    foliage: &BLOB_LARGE,
    log: Block::CherryLog,
    leaf: Block::CherryLeaves,
    height: (6, 9),
};

pub static OAK_SMALL: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_SMALL_F,
};
pub static OAK_SWAMP: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_SWAMP_F,
};
pub static OAK_BIG: ConfiguredFeature = ConfiguredFeature {
    feature: &OAK_BIG_F,
};
pub static REDWOOD: ConfiguredFeature = ConfiguredFeature {
    feature: &REDWOOD_F,
};
pub static SPRUCE: ConfiguredFeature = ConfiguredFeature { feature: &SPRUCE_F };
pub static BIRCH: ConfiguredFeature = ConfiguredFeature { feature: &BIRCH_F };
pub static JUNGLE: ConfiguredFeature = ConfiguredFeature { feature: &JUNGLE_F };
pub static ACACIA: ConfiguredFeature = ConfiguredFeature { feature: &ACACIA_F };
pub static DARK_OAK: ConfiguredFeature = ConfiguredFeature {
    feature: &DARK_OAK_F,
};
pub static CHERRY: ConfiguredFeature = ConfiguredFeature { feature: &CHERRY_F };

/// Pick the tree a placed sapling grows into when it matures (see
/// `world::sapling`). An oak sapling becomes the big fancy oak 20% of the time and
/// the ordinary small oak otherwise (the task's rule); every other sapling grows
/// into the single feature for its species. `rng` is drawn for the oak roll and
/// then consumed by the chosen feature's own geometry. A non-sapling block can't
/// reach here from the sapling behaviour; it falls back to the small oak.
pub fn sapling_tree(sapling: Block, rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match sapling {
        Block::OakSapling => {
            if rng.chance(0.2) {
                &OAK_BIG
            } else {
                &OAK_SMALL
            }
        }
        Block::SpruceSapling => &SPRUCE,
        Block::BirchSapling => &BIRCH,
        Block::JungleSapling => &JUNGLE,
        Block::AcaciaSapling => &ACACIA,
        Block::DarkOakSapling => &DARK_OAK,
        Block::CherrySapling => &CHERRY,
        _ => &OAK_SMALL,
    }
}
