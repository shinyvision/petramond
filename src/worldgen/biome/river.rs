use crate::biome::Biome;

use super::{surfaces, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::River,
    surface: &surfaces::OCEAN_FLOOR,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
};
