use crate::biome::Biome;
use crate::block::Block;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Swamp,
    surface: &surfaces::WETLAND_TOP,
    trees: TreeProfile::new(0.018, trees::swamp_oak),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.10),
    snow_cover: SnowCover::None,
};
