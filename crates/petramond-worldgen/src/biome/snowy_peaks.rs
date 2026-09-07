use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyPeaks,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::Always,
};
