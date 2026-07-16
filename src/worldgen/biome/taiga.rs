use crate::biome::Biome;
use crate::block::Block;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Taiga,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.026, trees::spruce),
    vegetation: VegetationProfile::grass(Block::Fern, 0.12),
    snow_cover: SnowCover::None,
};
