use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

const FLOWERS: &[Block] = &[
    Block::Dandelion,
    Block::Poppy,
    Block::OxeyeDaisy,
    Block::Cornflower,
    Block::AzureBluet,
];

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Plains,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.002, trees::plains_oak),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.14).with_flowers(FLOWERS, 0.1, 0.15),
};
