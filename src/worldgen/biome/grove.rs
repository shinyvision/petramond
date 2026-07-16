use crate::biome::Biome;
use crate::block::Block;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Grove,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.024, trees::spruce),
    vegetation: VegetationProfile::grass(Block::Fern, 0.08),
    snow_cover: SnowCover::Always,
};
