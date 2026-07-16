use crate::biome::Biome;
use crate::block::Block;
use crate::worldgen::rng::FeatureRng;

use super::{BiomeSpec, CoverCluster, SnowCover, TreeProfile, TreeSupport, VegetationProfile, surfaces, trees};

const FLOWERS: &[Block] = &[Block::OxeyeDaisy, Block::Poppy];

/// Salt for the fern-cluster patch field (distinct from the surface grass-cluster
/// field so the fern clumps and grass clumps don't coincide).
const FERN_PATCH_SALT: u64 = 0x0000_FE12_4E12_0001;

/// Lush within a clump (gated to clusters by `cover_cluster`), so ferns read as
/// dense clumps with bare podzol between rather than an even sprinkle. Fern stays
/// dominant, with short grass and the occasional brown/red mushroom mixed in.
fn ground_cover(rng: &mut FeatureRng) -> Option<Block> {
    if !rng.chance(0.55) {
        return None;
    }
    Some(match rng.next_i32(0, 99) {
        0..=69 => Block::Fern,
        70..=89 => Block::ShortGrass,
        90..=94 => Block::RedMushroom,
        _ => Block::BrownMushroom,
    })
}

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::RedwoodForest,
    surface: &surfaces::REDWOOD_TOP,
    trees: TreeProfile::new(0.12, trees::redwood_grove)
        .with_spacing(10)
        .with_height_clearance(56)
        .with_support(TreeSupport::RedwoodBase),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.0)
        .with_flowers(FLOWERS, 0.05, 0.14)
        .with_podzol_cover(ground_cover)
        .with_grass_cover(ground_cover)
        .with_cover_cluster(CoverCluster {
            salt: FERN_PATCH_SALT,
            period: 9.0,
            coverage: 0.5,
        }),
    snow_cover: SnowCover::None,
};
