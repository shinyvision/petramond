use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Wetland,
    surface: &surfaces::WETLAND_TOP,
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.10).with_hemp(0.016),
    snow_cover: SnowCover::None,
};
