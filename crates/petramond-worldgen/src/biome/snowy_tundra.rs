use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile, COLD_HEMP};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTundra,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::NONE.with_hemp(COLD_HEMP),
    snow_cover: SnowCover::Always,
};
