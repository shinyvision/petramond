use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Taiga,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.026, trees::spruce),
    vegetation: VegetationProfile::grass(Block::Fern, 0.12).with_hemp(0.0023),
    snow_cover: SnowCover::None,
};
