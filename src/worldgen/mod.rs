//! Worldgen pipeline — **Strata**.
//!
//! `generate_chunk(seed, cx, cz) -> Chunk` is the single deterministic
//! entrypoint, invoked in isolation on a worker thread (native pool / web
//! Worker) and serialized to flat block + per-column biome bytes.
//!
//! Strata phases (see `docs/ARCHITECTURE_WORLDGEN.md`):
//!   - P0: relocate the `gen.rs` god file into this module tree behind an ABI shim
//!   - P1: `noise` -> typed `HeightField` + const sampler settings
//!   - P2: a staged `driver::ChunkGenerator` + `ProtoChunk` + declarative
//!         surface rules + carvers + a `BiomeSource` (terrain now flows through
//!         the pipeline; features still use the legacy placer)
//!   - P3: a composable `feature` system (trunk/foliage placers) replacing `trees`
//!   - P4: cross-chunk margin + positional RNG + data-driven biome definitions

pub mod carve;
pub mod climate;
pub mod ctx;
pub mod data;
pub mod driver;
pub mod features;
pub mod noise;
pub mod proto;
pub mod rng;
pub mod surface;
pub mod trees;

pub use noise::WorldNoise;

use crate::chunk::Chunk;

/// Generate terrain + features for a chunk. Caller passes the world seed.
///
/// P2: terrain (fill + carve + surface) flows through the staged
/// `ChunkGenerator`; features are still layered on by the legacy placer until
/// P3 folds feature placement in as the pipeline's final stage.
pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    let generator = driver::ChunkGenerator::new(seed);
    let mut chunk = generator.generate(cx, cz);

    // Trees & features layered on top via deterministic RNG seeded per chunk.
    let noise = WorldNoise::new(seed);
    let mut frng = rng::FeatureRng::new(seed, cx, cz);
    features::place_features(&mut chunk, &noise, &mut frng);

    chunk.dirty = true;
    chunk
}
