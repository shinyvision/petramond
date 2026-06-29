use crate::biome::Biome;

use super::{surfaces, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::DeepOcean,
    surface: &surfaces::DEEP_OCEAN_FLOOR,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
};
