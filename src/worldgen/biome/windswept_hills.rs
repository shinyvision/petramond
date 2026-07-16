use crate::biome::Biome;
use crate::block::Block;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::WindsweptHills,
    surface: &surfaces::FOOTHILLS_TOP,
    trees: TreeProfile::new(0.004, trees::oak_small),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.05),
    snow_cover: SnowCover::None,
};
