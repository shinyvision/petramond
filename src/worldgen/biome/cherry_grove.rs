use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

const FLOWERS: &[Block] = &[Block::Poppy, Block::Dandelion, Block::OxeyeDaisy];

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::CherryGrove,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.035, trees::cherry),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.13).with_flowers(FLOWERS, 0.22, 0.26),
};
