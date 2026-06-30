//! Worldgen pipeline.
//!
//! `generate_chunk(seed, cx, cz) -> Chunk` is the single deterministic
//! entrypoint, invoked in isolation on a worker thread (native pool / web
//! Worker) and serialized to flat block + per-column biome bytes.
//!
//! Active terrain is built from the surface density graph: climate graph biome
//! assignment, `master_density` sign fill, sea-level water, exposed-run surface
//! skinning, cave carving, underground scatter, ground vegetation, and tree
//! features.

pub(crate) mod audit;
pub(crate) mod biome;
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
    generator.carve_caves(&mut chunk);
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

    /// The cubic per-section generator must be byte-identical, above ground, to the
    /// whole-column generator: assembling `generate_section` over a column's surface
    /// sections (cy 0..15) reproduces `generate_chunk`'s blocks and biomes exactly.
    /// This is the S3 correctness gate — terrain, scatter, vegetation, and trees all
    /// clip per-section without drift across the (now 3D) seams.
    #[test]
    fn per_section_generation_matches_whole_column_above_ground() {
        use crate::chunk::{SectionPos, CHUNK_SY, SECTION_SIZE};

        let seed = 0x1234_5678;
        let generator = driver::ChunkGenerator::new(seed);
        for &(cx, cz) in &[(0, 0), (1, -1), (-3, 5), (12, -7), (4, -3)] {
            let chunk = generate_chunk(seed, cx, cz);
            let col = generator.generate_column_gen(cx, cz);

            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    assert_eq!(
                        col.biome_at(x, z),
                        chunk.biome_at(x, z),
                        "biome mismatch at ({cx},{cz}) col ({x},{z})"
                    );
                }
            }

            for cy in 0..(CHUNK_SY / SECTION_SIZE) as i32 {
                let section = generator.generate_section(SectionPos::new(cx, cy, cz), &col);
                for ly in 0..SECTION_SIZE {
                    let wy = cy as usize * SECTION_SIZE + ly;
                    for z in 0..CHUNK_SZ {
                        for x in 0..CHUNK_SX {
                            assert_eq!(
                                section.block_raw(x, ly, z),
                                chunk.block_raw(x, wy, z),
                                "block mismatch at ({cx},{cz}) cy {cy} local ({x},{ly},{z}) world y {wy}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn cave_capable_section_summaries_are_conservative() {
        use super::noise::height::CaveField;
        use crate::section::SectionSummary;

        let seed = 0x1234_5678;
        let generator = driver::ChunkGenerator::new(seed);
        let mut checked = 0;

        for &(cx, cz) in &[(0, 0), (1, -1), (-3, 5), (12, -7), (4, -3)] {
            let col = generator.generate_column_gen(cx, cz);
            let (surf_min, surf_max) = col.surf_range();
            for cy in -4..=15 {
                if CaveField::section_may_carve(cy, surf_min, surf_max) {
                    checked += 1;
                    assert_eq!(
                        col.section_summary(cy),
                        SectionSummary::Mixed,
                        "cave-capable generated section must be mixed at ({cx},{cy},{cz})"
                    );
                }
            }
        }

        assert!(
            checked > 0,
            "test must exercise at least one cave-capable section"
        );
    }

    /// Generating sections across a wide area must not corrupt the random-tick gate.
    /// `fill_section` writes the block buffer in bulk, then the scatter/vegetation/tree
    /// stages edit through the setters — so a tree trunk overwriting a random-tickable
    /// skin block (surface grass) used to underflow the still-zero counter (panic in
    /// debug, silent wrap in release). After generation the count must equal a
    /// from-scratch tally of the section's random-tickable blocks.
    #[test]
    fn per_section_generation_keeps_random_tick_count_exact() {
        use crate::block::Block;
        use crate::chunk::{SectionPos, CHUNK_SY, SECTION_SIZE};

        for &seed in &[1u32, 7, 42, 0x1234_5678] {
            let generator = driver::ChunkGenerator::new(seed);
            for cz in -3..=3 {
                for cx in -3..=3 {
                    let col = generator.generate_column_gen(cx, cz);
                    for cy in 0..(CHUNK_SY / SECTION_SIZE) as i32 {
                        let section = generator.generate_section(SectionPos::new(cx, cy, cz), &col);
                        let expected = section
                            .blocks_slice()
                            .iter()
                            .filter(|&&id| Block::from_id(id).has_random_tick())
                            .count() as u32;
                        assert_eq!(
                            section.random_tick_count(),
                            expected,
                            "random-tick count drifted at ({cx},{cy},{cz}) seed {seed:#x}"
                        );
                    }
                }
            }
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
