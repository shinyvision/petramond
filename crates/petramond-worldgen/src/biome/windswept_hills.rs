use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::WindsweptHills,
    surface: &surfaces::FOOTHILLS_TOP,
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.05),
    snow_cover: SnowCover::None,
};
