use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::WoodedHills,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.040, trees::forest_oak),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.09),
};
