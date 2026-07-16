use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTundra,
    surface: &surfaces::PLAINS_TOP,
    // Scattered lone spruces on open snow — between the treeless SnowyPlains
    // and the SnowyTaiga spruce forest.
    trees: TreeProfile::new(0.004, trees::spruce),
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::Always,
};
