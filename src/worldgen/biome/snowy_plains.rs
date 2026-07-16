use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces};

/// Open snowfield: the treeless cold flat. Distinct from snowy tundra
/// (scattered lone spruces) and snowy taiga (spruce forest).
pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyPlains,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::Always,
};
