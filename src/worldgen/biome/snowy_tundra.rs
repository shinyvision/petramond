use crate::biome::Biome;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile, COLD_HEMP};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTundra,
    surface: &surfaces::PLAINS_TOP,
    // Scattered lone spruces on open snow — between the treeless SnowyPlains
    // and the SnowyTaiga spruce forest.
    trees: TreeProfile::new(0.004, trees::spruce),
    vegetation: VegetationProfile::NONE.with_hemp(COLD_HEMP),
    snow_cover: SnowCover::Always,
};
