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
    // Genuinely rare: a lone landmark oak every few hundred blocks — the
    // tuned oaks are big enough that more would crowd the open plain.
    trees: TreeProfile::new(0.0002, trees::plains_oak).with_height_clearance(30),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.14).with_flowers(FLOWERS, 0.1, 0.15),
};
