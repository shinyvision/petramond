use crate::rng::FeatureRng;
use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::{surfaces, BiomeSpec, SnowCover, TreeProfile, VegetationProfile};

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
    snow_cover: SnowCover::None,
};
