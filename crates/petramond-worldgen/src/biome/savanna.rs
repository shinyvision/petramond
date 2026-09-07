use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Savanna,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.14).with_hemp(0.0023),
    snow_cover: SnowCover::None,
};
