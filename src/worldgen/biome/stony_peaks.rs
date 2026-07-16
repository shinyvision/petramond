use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::StonyPeaks,
    surface: &surfaces::STONY_TOP,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::None,
};
