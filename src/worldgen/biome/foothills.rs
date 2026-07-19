use crate::biome::Biome;

use super::{surfaces, trees, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Foothills,
    surface: &surfaces::FOOTHILLS_TOP,
    trees: TreeProfile::new(0.012, trees::oak_small),
    vegetation: VegetationProfile::grass(crate::block::Block::ShortGrass, 0.06),
    snow_cover: SnowCover::None,
};
