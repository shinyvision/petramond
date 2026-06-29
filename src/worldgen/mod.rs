//! Worldgen pipeline.
//!
//! `generate_chunk(seed, cx, cz) -> Chunk` is the single deterministic
//! entrypoint, invoked in isolation on a worker thread (native pool / web
//! Worker) and serialized to flat block + per-column biome bytes.
//!
//! Active terrain is built from the surface density graph: climate graph biome
//! assignment, `master_density` sign fill, sea-level water, exposed-run surface
//! skinning, underground scatter, ground vegetation, and tree features. Caves are
//! not composed into the live density fill path.

pub(crate) mod audit;
pub(crate) mod biome;
mod ctx;
pub(crate) mod data;
pub(crate) mod density;
pub(crate) mod driver;
pub(crate) mod feature;
pub(crate) mod graph;
mod noise;
mod proto;
pub(crate) mod region;
pub(crate) mod rng;
pub(crate) mod spawn;
mod surface;

use crate::chunk::Chunk;

/// Generate terrain + features for a chunk. Caller passes the world seed.
///
/// Terrain and feature placement both flow through the staged `ChunkGenerator`.
/// Features are placed via world-positional RNG over the chunk plus a margin
/// border, so trees cross chunk seams seamlessly.
///
/// The generator holds only immutable seed-derived state (noise samplers and
/// worldgen subsystems), which is expensive to build, so it is cached per thread
/// keyed by seed — repeated one-shot calls for the same world reuse it instead of
/// rebuilding the pipeline per chunk. Hot worker loops should still hold their
/// own generator and call [`generate_chunk_with`] directly.
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
    let mut chunk = generator.generate_surface(cx, cz);
    generator.place_underground(&mut chunk);
    generator.place_vegetation(&mut chunk);
    generator.place_features_runtime(&mut chunk);

    chunk.dirty = true;
    chunk
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{CHUNK_SX, CHUNK_SZ, SEA_LEVEL};

    /// A generator whose shared noise cache has been warmed by neighbouring chunks
    /// must produce byte-identical output to a generator that computes every column
    /// fresh — proving the cache only memoizes and never affects results, whatever
    /// order chunks are generated in (the property the worker pool relies on).
    #[test]
    fn shared_noise_cache_does_not_change_output() {
        let seed = 0x1234_5678;
        let warmed = driver::ChunkGenerator::new(seed);
        // Warm one generator with a spread of chunks before comparing against a
        // fresh generator. Generation state must remain immutable and pure.
        for cz in -2..=2 {
            for cx in -2..=2 {
                let _ = generate_chunk_with(&warmed, cx, cz);
            }
        }
        let fresh = driver::ChunkGenerator::new(seed); // independent private cache

        for (cx, cz) in [(0, 0), (1, -1), (2, 2), (-2, 1), (10, -8)] {
            let warm_chunk = generate_chunk_with(&warmed, cx, cz);
            let fresh_chunk = generate_chunk_with(&fresh, cx, cz);
            assert_eq!(
                warm_chunk.blocks_slice(),
                fresh_chunk.blocks_slice(),
                "blocks differ with warm cache at ({cx},{cz})"
            );
            assert_eq!(
                &warm_chunk.heightmap[..],
                &fresh_chunk.heightmap[..],
                "heightmap differs with warm cache at ({cx},{cz})"
            );
        }
    }

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
