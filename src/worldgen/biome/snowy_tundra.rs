use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTundra,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.003, trees::oak_small),
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::Always,
};
