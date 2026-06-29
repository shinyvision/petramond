use crate::biome::Biome;
use crate::block::Block;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::SnowyTaiga,
    surface: &surfaces::SNOW_TOP,
    trees: TreeProfile::new(0.020, trees::spruce),
    vegetation: VegetationProfile::grass(Block::Fern, 0.12),
};
