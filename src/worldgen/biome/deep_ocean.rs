use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::DeepOcean,
    surface: &surfaces::DEEP_OCEAN_FLOOR,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::None,
};
