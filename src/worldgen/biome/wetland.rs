use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Wetland,
    surface: &surfaces::WETLAND_TOP,
    trees: TreeProfile::new(0.011, trees::wetland_oak),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.10),
};
