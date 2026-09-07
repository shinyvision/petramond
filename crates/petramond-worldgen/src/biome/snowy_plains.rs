use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile, COLD_HEMP};

/// Open snowfield: the treeless cold flat. Distinct from snowy tundra
/// (scattered lone spruces) and snowy taiga (spruce forest).
pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyPlains,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::NONE.with_hemp(COLD_HEMP),
    snow_cover: SnowCover::Always,
};
