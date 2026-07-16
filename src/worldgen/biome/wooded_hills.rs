use crate::biome::Biome;
use crate::block::Block;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::WoodedHills,
    surface: &surfaces::PLAINS_TOP,
    // Sparser to fit the tuned oaks' footprint (see forest.rs).
    trees: TreeProfile::new(0.004, trees::forest_oak)
        .with_spacing(10)
        .with_height_clearance(30),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.09),
    snow_cover: SnowCover::None,
};
