use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

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
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.14)
        .with_flowers(FLOWERS, 0.1, 0.15)
        .with_hemp(0.0045),
    snow_cover: SnowCover::None,
};
