use crate::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Ocean,
    surface: &surfaces::OCEAN_FLOOR,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::None,
};
