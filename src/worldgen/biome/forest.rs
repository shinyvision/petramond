use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

const FLOWERS: &[Block] = &[Block::Poppy, Block::Dandelion, Block::OxeyeDaisy];

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Forest,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.055, trees::forest_oak),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.11).with_flowers(FLOWERS, 0.16, 0.22),
};
