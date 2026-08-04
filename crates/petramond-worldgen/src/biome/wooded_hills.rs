use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::WoodedHills,
    surface: &surfaces::PLAINS_TOP,
    // Sparser to fit the tuned oaks' footprint (see forest.rs).
    trees: TreeProfile::new(0.004, trees::forest_oak)
        .with_spacing(10)
        .with_height_clearance(30),
    vegetation: VegetationProfile::grass(Block::ShortGrass, 0.09).with_hemp(0.0037),
    snow_cover: SnowCover::None,
};
