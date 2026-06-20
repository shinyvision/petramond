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

pub mod classic;
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
/// Terrain (fill + carve + surface) and feature placement both flow through the
/// staged `ChunkGenerator`. P4: features are placed via world-positional RNG
/// over the chunk + a margin border, so trees cross chunk seams seamlessly.
///
/// The generator holds only immutable seed-derived state (noise samplers + the
/// cascade layer stacks), which is expensive to build, so it is cached per thread
/// keyed by seed — repeated one-shot calls for the same world reuse it instead of
/// rebuilding every cascade layer per chunk. Hot worker loops should still hold
/// their own generator and call [`generate_chunk_with`] directly.
pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    thread_local! {
        static CACHED: std::cell::RefCell<Option<(u32, driver::ChunkGenerator)>> =
            const { std::cell::RefCell::new(None) };
    }
    CACHED.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().map(|(s, _)| *s) != Some(seed) {
            *slot = Some((seed, driver::ChunkGenerator::new(seed)));
        }
        generate_chunk_with(&slot.as_ref().unwrap().1, cx, cz)
    })
}

/// Generate terrain + features with an already-built generator.
///
/// This preserves `generate_chunk` as the public one-shot API while allowing
/// hot worker loops to reuse the generator's immutable seed-derived state.
pub fn generate_chunk_with(generator: &driver::ChunkGenerator, cx: i32, cz: i32) -> Chunk {
    // The cascade region (chunk + feature margin) is computed ONCE and shared by
    // terrain fill and feature placement — the whole chunk's biomes + biome-driven
    // height are generated a single time.
    let region = generator.region(cx, cz);
    let mut chunk = generator.generate(&region, cx, cz);
    generator.place_underground(&mut chunk);
    generator.place_vegetation(&mut chunk);
    generator.place_features(&mut chunk, &region);

    chunk.dirty = true;
    chunk
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{CHUNK_SX, CHUNK_SZ, SEA_LEVEL};

    #[test]
    fn generate_chunk_with_matches_one_shot() {
        let seed = 0x1234_5678;
        let generator = driver::ChunkGenerator::new(seed);

        for (cx, cz) in [(0, 0), (-3, 5), (12, -7)] {
            let one_shot = generate_chunk(seed, cx, cz);
            let reused = generate_chunk_with(&generator, cx, cz);

            assert_eq!(one_shot.cx, reused.cx);
            assert_eq!(one_shot.cz, reused.cz);
            assert_eq!(one_shot.blocks_slice(), reused.blocks_slice());
            assert_eq!(one_shot.biomes_slice(), reused.biomes_slice());
            assert_eq!(&one_shot.heightmap[..], &reused.heightmap[..]);
            assert_eq!(one_shot.dirty, reused.dirty);
            assert_eq!(one_shot.light_dirty, reused.light_dirty);
        }
    }

    #[test]
    fn generated_underwater_terrain_has_no_grass_blocks() {
        for &seed in &[0x1234_5678u32, 1, 0xDEAD_BEEF, 7] {
            for cz in -3..=3 {
                for cx in -3..=3 {
                    let chunk = generate_chunk(seed, cx, cz);
                    for z in 0..CHUNK_SZ {
                        for x in 0..CHUNK_SX {
                            for y in 0..SEA_LEVEL as usize {
                                let block = chunk.block(x, y, z);
                                assert!(
                                    !matches!(block, Block::Grass | Block::Snow),
                                    "grass variant {block:?} below sea level at chunk ({cx},{cz}) local ({x},{y},{z}) seed {seed:#x}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
