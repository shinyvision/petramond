use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

const FLOWERS: &[Block] = &[Block::Poppy, Block::Dandelion, Block::OxeyeDaisy];

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Forest,
    surface: &surfaces::PLAINS_TOP,
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.11)
        .with_flowers(FLOWERS, 0.16, 0.22)
        .with_hemp(0.0037),
    snow_cover: SnowCover::None,
};
