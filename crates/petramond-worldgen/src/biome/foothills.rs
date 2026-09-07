use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Foothills,
    surface: &surfaces::FOOTHILLS_TOP,
    vegetation: VegetationProfile::grass(petramond_world::block::Block::ShortGrass, 0.06),
    snow_cover: SnowCover::None,
};
