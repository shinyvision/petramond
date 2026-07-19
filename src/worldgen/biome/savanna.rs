use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Savanna,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.015, trees::acacia),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.14),
    snow_cover: SnowCover::None,
};
