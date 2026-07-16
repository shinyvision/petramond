use crate::biome::Biome;

use super::{BiomeSpec, SnowCover, TreeProfile, VegetationProfile, surfaces, trees};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Foothills,
    surface: &surfaces::FOOTHILLS_TOP,
    trees: TreeProfile::new(0.012, trees::oak_small),
    vegetation: VegetationProfile::grass(crate::block::Block::ShortGrass, 0.06),
    snow_cover: SnowCover::None,
};
