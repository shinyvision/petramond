use crate::biome::Biome;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTundra,
    surface: &surfaces::SNOW_TOP,
    trees: TreeProfile::new(0.003, trees::oak_small),
    vegetation: VegetationProfile::NONE,
};
