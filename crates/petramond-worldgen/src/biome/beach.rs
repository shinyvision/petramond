use petramond_world::biome::Biome;

use super::{surfaces, BiomeSpec, SnowCover, VegetationProfile};

// Beaches are barren sand in the reference generator: no trees and no sand
// cover. Cactus and dead bush belong to the arid biomes (desert / desert lakes),
// not the temperate coast — placing them here made beaches read as tiny deserts.
pub(super) static SPEC: BiomeSpec = BiomeSpec {
    biome: Biome::Beach,
    surface: &surfaces::SAND_DEEP,
    vegetation: VegetationProfile::NONE,
    snow_cover: SnowCover::None,
};
