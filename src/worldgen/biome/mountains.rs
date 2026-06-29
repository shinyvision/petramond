use crate::biome::Biome;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Mountains,
    surface: &surfaces::MOUNTAIN_TOP,
    trees: TreeProfile::new(0.004, trees::oak_small),
    vegetation: VegetationProfile::grass(crate::block::Block::ShortGrass, 0.05),
};
