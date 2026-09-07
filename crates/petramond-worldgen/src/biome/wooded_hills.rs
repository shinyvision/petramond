use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::WoodedHills,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.09).with_hemp(0.0037),
    snow_cover: SnowCover::None,
};
