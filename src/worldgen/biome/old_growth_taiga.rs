use crate::biome::Biome;
use crate::block::Block;
use crate::worldgen::rng::FeatureRng;

use super::{surfaces, trees, BiomeSpec, TreeProfile, VegetationProfile};

fn podzol_cover(rng: &mut FeatureRng) -> Option<Block> {
    if !rng.chance(0.10) {
        return None;
    }
    let r = rng.next_i32(0, 99);
    Some(if r < 58 {
        Block::Fern
    } else if r < 80 {
        Block::ShortGrass
    } else if r < 90 {
        Block::RedMushroom
    } else {
        Block::BrownMushroom
    })
}

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::OldGrowthTaiga,
    surface: &surfaces::PODZOL_TOP,
    trees: TreeProfile::new(0.035, trees::spruce),
    vegetation: VegetationProfile::grass(Block::Fern, 0.12).with_podzol_cover(podzol_cover),
};
