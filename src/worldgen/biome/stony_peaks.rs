use crate::biome::Biome;

use super::{surfaces, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::StonyPeaks,
    surface: &surfaces::STONY_TOP,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
};
