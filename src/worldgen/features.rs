//! Per-chunk feature placement (trees today).
//!
//! Strata P0: relocated verbatim from `gen.rs` `mod features`, with the internal
//! `crate::gen::*` paths repointed to `super::*`. P3 replaces the bespoke
//! `tree_probability`/`pick_oak_variant` dispatch with data-driven
//! `PlacedFeature` rows; P4 removes the chunk-edge skip via a bordered
//! `ProtoChunk` and positional RNG.

use crate::biome::{Biome, biome_at};
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use super::rng::FeatureRng;
use super::WorldNoise;
use super::trees;

pub fn place_features(chunk: &mut Chunk, noise: &WorldNoise, rng: &mut FeatureRng) {
    // Trees: per-column probability, biome-dependent.
    let (ox, oz) = chunk.chunk_origin_world();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            if x == 0 || z == 0 || x == CHUNK_SX - 1 || z == CHUNK_SZ - 1 {
                // Avoid trees at chunk edges to minimise cross-chunk
                // collisions (true impl would query neighbours).
                continue;
            }
            let surf = noise.surface_height(wx, wz);
            if surf <= SEA_LEVEL { continue; }
            let climate = noise.climate(wx, wz);
            let biome = biome_at(climate, surf);
            let p = tree_probability(biome);
            if !rng.chance(p) { continue; }

            let variant = pick_oak_variant(rng, biome);
            place_oak(chunk, x, surf, z, variant, rng);
        }
    }
}

fn tree_probability(b: Biome) -> f32 {
    match b {
        Biome::Forest => 0.06,
        Biome::BirchForest => 0.04,
        Biome::Plains => 0.012,
        Biome::Savanna => 0.015,
        Biome::Swamp => 0.014,
        Biome::Taiga => 0.010,
        Biome::SnowyTaiga => 0.010,
        Biome::SnowyTundra => 0.002,
        _ => 0.0,
    }
}

fn pick_oak_variant(rng: &mut FeatureRng, b: Biome) -> trees::OakVariant {
    use trees::OakVariant::*;
    // Distribution biased by biome.
    match b {
        Biome::Forest => match rng.next_i32(0, 99) {
            0..=4 => OakBig,
            5..=44 => Oak2,
            45..=74 => Oak3,
            _ => Oak1,
        },
        Biome::Plains | Biome::Savanna => match rng.next_i32(0, 99) {
            0..=2 => OakBig,
            3..=72 => Oak1,
            _ => Oak4,
        },
        Biome::Swamp => match rng.next_i32(0, 99) {
            0..=9 => OakBig,
            _ => Oak4,
        },
        _ => match rng.next_i32(0, 99) {
            0..=2 => OakBig,
            _ => Oak1,
        },
    }
}

fn place_oak(
    chunk: &mut Chunk, x: usize, y: i32, z: usize,
    variant: trees::OakVariant, rng: &mut FeatureRng,
) {
    if y < 1 || y + 12 >= CHUNK_SY as i32 { return; }
    trees::place(chunk, x, y, z, variant, rng);
}
