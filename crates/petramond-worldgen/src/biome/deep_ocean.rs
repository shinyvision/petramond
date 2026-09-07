use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::DeepOcean,
    surface: &surfaces::DEEP_OCEAN_FLOOR,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::None,
};
