//! Shared tree variant pickers selected by biome specs.

use crate::worldgen::data::features;
use crate::worldgen::feature::ConfiguredFeature;
use crate::worldgen::rng::FeatureRng;

pub(crate) fn oak_small(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=39 => &features::OAK_YOUNG,
        _ => &features::OAK_SMALL,
    }
}

/// Forest age mix: mostly standard oaks, a scattering of young trees between
/// them, and the occasional grand landmark oak.
pub(crate) fn forest_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=11 => &features::OAK_BIG,
        12..=41 => &features::OAK_YOUNG,
        _ => &features::OAK_SMALL,
    }
}

pub(crate) fn plains_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=19 => &features::OAK_BIG,
        20..=44 => &features::OAK_YOUNG,
        _ => &features::OAK_SMALL,
    }
}

pub(crate) fn acacia(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    let _ = rng.next_i32(0, 99);
    &features::ACACIA
}

pub(crate) fn spruce(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    let _ = rng.next_i32(0, 99);
    &features::SPRUCE
}

pub(crate) fn swamp_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=14 => &features::OAK_BIG,
        _ => &features::OAK_SWAMP,
    }
}

pub(crate) fn wetland_oak(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    match rng.next_i32(0, 99) {
        0..=29 => &features::OAK_SMALL,
        _ => &features::OAK_SWAMP,
    }
}

/// Redwood grove: mostly giant redwoods with the occasional spruce mixed in. The
/// single `next_i32(0, 99)` draw keeps the 75% that stay redwood on the same
/// geometry stream they'd have if the species were never rolled.
pub(crate) fn redwood_grove(rng: &mut FeatureRng) -> &'static ConfiguredFeature {
    if rng.next_i32(0, 99) < 25 {
        &features::SPRUCE
    } else {
        &features::REDWOOD
    }
}

