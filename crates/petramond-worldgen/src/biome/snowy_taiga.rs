use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile, COLD_HEMP};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTaiga,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::grass(Block::Fern, 0.12).with_hemp(COLD_HEMP),
    snow_cover: SnowCover::Always,
};
