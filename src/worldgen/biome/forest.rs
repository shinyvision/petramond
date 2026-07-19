use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

const FLOWERS: &[Block] = &[Block::Poppy, Block::Dandelion, Block::OxeyeDaisy];

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Forest,
    surface: &surfaces::PLAINS_TOP,
    // Sparse relative to the pre-concept-oak forest (0.055, spacing 3): the
    // tuned oaks are up to ~30 blocks wide, so few, well-spaced trees keep
    // the forest walkable while the canopies still touch.
    trees: TreeProfile::new(0.005, trees::forest_oak)
        .with_spacing(10)
        .with_height_clearance(30),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.11).with_flowers(FLOWERS, 0.16, 0.22),
    snow_cover: SnowCover::None,
};
