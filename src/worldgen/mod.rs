//! Worldgen pipeline.
//!
//! `generate_chunk(seed, cx, cz) -> Chunk` is the single deterministic
//! entrypoint, invoked in isolation on a worker thread (native pool / web
//! Worker) and serialized to flat block + per-column biome bytes.
//!
//! Active terrain is built from the classic land-biome terrain provider, then an
//! explicit river path system carves channels and water levels, followed by
//! surface skinning, underground scatter, ground vegetation, and tree features.

pub mod audit;
pub mod classic;
pub mod climate;
pub mod ctx;
pub mod data;
pub mod driver;
pub mod feature;
pub mod noise;
pub mod proto;
pub mod river;
pub mod rng;
pub mod spawn;
pub mod surface;

use crate::chunk::Chunk;

/// Generate terrain + features for a chunk. Caller passes the world seed.
///
/// Terrain and feature placement both flow through the staged `ChunkGenerator`.
/// Features are placed via world-positional RNG over the chunk plus a margin
/// border, so trees cross chunk seams seamlessly.
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
    // The region (chunk + feature margin) is computed ONCE and shared by terrain
    // fill and feature placement, including river-carved surface metadata.
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

    /// A generator whose shared noise cache has been warmed by neighbouring chunks
    /// must produce byte-identical output to a generator that computes every column
    /// fresh — proving the cache only memoizes and never affects results, whatever
    /// order chunks are generated in (the property the worker pool relies on).
    #[test]
    fn shared_noise_cache_does_not_change_output() {
        use crate::worldgen::classic::terrain::NoiseCache;
        use std::sync::Arc;

        let seed = 0x1234_5678;
        let warmed = driver::ChunkGenerator::with_cache(seed, Arc::new(NoiseCache::new()));
        // Warm the cache with a spread of chunks so the target chunks' lattice
        // columns are served from the cache (incl. promotion paths).
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

    /// Frozen golden for worldgen determinism. FNV-1a-folds every byte that
    /// `generate_chunk` is responsible for -- block ids, per-column biome ids, AND
    /// the water/fluid-flow metadata (`water_slice`) -- across 3 seeds x the 3x3
    /// chunk grid into one combined hash, pinned to a literal captured from the
    /// current baseline. Any change to generation output (or to the water-meta a
    /// chunk emits) flips this, catching a consistent-but-wrong result that the
    /// self-consistency tests above cannot. Mirrors the FNV scheme in
    /// `src/bin/genparity.rs`.
    ///
    /// The constant happens to equal that bin's blocks+biomes-only COMBINED
    /// (0x1072f2452379aff5): at *generation* time no column has yet emitted
    /// flowing-water metadata (oceans/rivers are all-source, so `water_slice()` is
    /// `None`), and folding the empty water slice is a no-op. The water-meta byte
    /// is folded regardless so the moment generation starts emitting flow meta this
    /// golden diverges and must be re-captured.
    #[test]
    fn generate_chunk_golden_is_byte_stable() {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

        fn fnv1a(bytes: &[u8], mut h: u64) -> u64 {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        }

        const SEEDS: [u32; 3] = [0x1234_5678, 1, 0xDEAD_BEEF];
        const COORDS: [(i32, i32); 9] = [
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 0),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ];

        let mut combined = FNV_OFFSET;
        for &seed in &SEEDS {
            for &(cx, cz) in &COORDS {
                let chunk = generate_chunk(seed, cx, cz);
                let mut h = FNV_OFFSET;
                h = fnv1a(chunk.blocks_slice(), h);
                h = fnv1a(chunk.biomes_slice(), h);
                // Fold the water-flow metadata. `None` (the column never held
                // flowing water) folds an empty slice -- folding nothing -- which
                // is deterministic and distinct from an all-zero `Some`.
                h = fnv1a(chunk.water_slice().unwrap_or(&[]), h);
                combined = fnv1a(&h.to_le_bytes(), combined);
            }
        }

        assert_eq!(
            combined, 0x1072_f245_2379_aff5,
            "worldgen (blocks + biomes + water-meta) byte output changed"
        );
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
