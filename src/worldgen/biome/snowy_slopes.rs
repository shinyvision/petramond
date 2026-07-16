use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowySlopes,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::Always,
};
