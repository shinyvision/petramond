use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Mountains,
    surface: &surfaces::MOUNTAIN_TOP,
    vegetation: VegetationProfile::grass(petramond_world::block::Block::ShortGrass, 0.05),
    snow_cover: SnowCover::AboveSurfaceY(surfaces::MOUNTAIN_SNOW_LINE),
};
