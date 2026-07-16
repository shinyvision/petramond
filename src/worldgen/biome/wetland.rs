use crate::biome::Biome;
use crate::block::Block;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Wetland,
    surface: &surfaces::WETLAND_TOP,
    trees: TreeProfile::new(0.011, trees::wetland_oak),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.10),
    snow_cover: SnowCover::None,
};
