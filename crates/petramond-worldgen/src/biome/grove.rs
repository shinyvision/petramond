use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile, COLD_HEMP};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Grove,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.024, trees::spruce),
    vegetation: VegetationProfile::grass(Block::Fern, 0.08).with_hemp(COLD_HEMP),
    snow_cover: SnowCover::Always,
};
