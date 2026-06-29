use crate::biome::Biome;
use crate::block::Block;
use crate::worldgen::rng::FeatureRng;

use super::{surfaces, BiomeSpec, TreeProfile, VegetationProfile};

fn sand_cover(rng: &mut FeatureRng) -> Option<Block> {
    if !rng.chance(0.007) {
        return None;
    }
    Some(if rng.next_i32(0, 99) < 45 {
        Block::DeadBush
    } else {
        Block::Cactus
    })
}

pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::DesertLakes,
    surface: &surfaces::SAND_DEEP,
    trees: TreeProfile::NONE,
    vegetation: VegetationProfile::NONE.with_sand_cover(sand_cover),
};
