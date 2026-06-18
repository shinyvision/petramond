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
pub mod feature;
pub mod noise;
pub mod proto;
pub mod rng;
pub mod surface;

pub use noise::WorldNoise;

use crate::chunk::Chunk;

/// Generate terrain + features for a chunk. Caller passes the world seed.
///
/// P3: terrain (fill + carve + surface) and feature placement both flow through
/// the staged `ChunkGenerator`; the composable `feature` system replaces the
/// bespoke oak functions.
pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    let generator = driver::ChunkGenerator::new(seed);
    let mut chunk = generator.generate(cx, cz);
    generator.place_features(&mut chunk, cx, cz);

    chunk.dirty = true;
    chunk
}
