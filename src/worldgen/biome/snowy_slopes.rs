use crate::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowySlopes,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::Always,
};
