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
    biome: Biome::Meadow,
    surface: &surfaces::PLAINS_TOP,
    trees: TreeProfile::new(0.006, trees::oak_small),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.16).with_flowers(FLOWERS, 0.36, 0.34),
};
