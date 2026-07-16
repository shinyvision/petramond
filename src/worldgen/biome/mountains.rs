use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Mountains,
    surface: &surfaces::MOUNTAIN_TOP,
    trees: TreeProfile::new(0.004, trees::oak_small),
    vegetation: VegetationProfile::grass(crate::block::Block::ShortGrass, 0.05),
    snow_cover: SnowCover::AboveSurfaceY(surfaces::MOUNTAIN_SNOW_LINE),
};
