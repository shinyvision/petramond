//! Shared tree variant pickers selected by biome specs.

use crate::worldgen::data::features;
use crate::worldgen::feature::ConfiguredFeature;
use crate::worldgen::rng::FeatureRng;

pub(crate) fn oak_small(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=39 => features::oak_young(),
        _ => features::oak_small(),
    }
}

/// Forest age mix: mostly standard oaks, a scattering of young trees between
/// them, and the occasional grand landmark oak.
pub(crate) fn forest_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=11 => features::oak_big(),
        12..=41 => features::oak_young(),
        _ => features::oak_small(),
    }
}

pub(crate) fn plains_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=19 => features::oak_big(),
        20..=44 => features::oak_young(),
        _ => features::oak_small(),
    }
}

pub(crate) fn acacia(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    let _ = rng.next_i32(0, 99);
    features::acacia()
}

pub(crate) fn spruce(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    let _ = rng.next_i32(0, 99);
    features::spruce()
}

pub(crate) fn swamp_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=14 => features::oak_big(),
        _ => features::oak_swamp(),
    }
}

pub(crate) fn wetland_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=29 => features::oak_small(),
        _ => features::oak_swamp(),
    }
}

/// Redwood grove: mostly giant redwoods with the occasional spruce mixed in. The
/// single `next_i32(0, 99)` draw keeps the 75% that stay redwood on the same
/// geometry stream they'd have if the species were never rolled.
pub(crate) fn redwood_grove(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    if rng.next_i32(0, 99) < 25 {
        features::spruce()
    } else {
        features::redwood()
    }
}
