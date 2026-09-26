//! Worldgen pipeline.
//!
//! One pipeline, per section: [`driver::ChunkGenerator`] computes a column's
//! shared data once, then generates each 16³ section — the streamer's unit —
//! in a fixed stage order. `generate_chunk(seed, cx, cz) -> Chunk` assembles a
//! chunk column from those same sections for tooling and the parity hash.
//!
//! Active terrain is built from the surface density graph: climate graph biome
//! assignment, `master_density` sign fill, sea-level water, exposed-run surface
//! skinning, cave carving, underground scatter, ground vegetation, and tree
//! features.

// The audits back slow `#[ignore]`d tests too, so tests compile them always.
#[cfg(any(test, feature = "tools"))]
pub mod audit;
pub mod biome;
pub mod cache;
pub mod colgen;
pub mod data;
pub mod density;
pub mod driver;
pub mod feature;
pub mod formula;
pub mod graph;
pub mod hooks;
mod noise;
pub mod parity;
mod queries;
#[cfg(feature = "tools")]
pub mod preview;
pub mod region;
pub mod rng;
pub(crate) mod salts;
mod section_memo;
pub mod spawn;
mod surface;

use petramond_world::chunk::Chunk;

// The facade: what the engine, server and client use. Generation runs through
// `ChunkGenerator`; positional questions go through the queries below.
pub use density::surface::SurfaceDensitySystem;
pub use driver::{ChunkGenerator, ColumnGen, PendingSection, SectionGen};
pub use mod_api::TerrainSpace;
pub(crate) use queries::UndergroundBoxKey;
pub use queries::{
    blocks_at as terrain_blocks_at, heights_at as terrain_heights_at,
    section_blocks as terrain_section_at, surface_biome_at, terrain_solid_at, terrain_space_at,
    underground_biomes_at, underground_biomes_in_box,
};
pub use rng::FeatureRng;

/// Runtime feature placement outside the generation pipeline — sapling
/// growth: resolve a configured feature by name and place it through a
/// [`VoxelSink`](growth::VoxelSink) over the live world.
pub mod growth {
    pub use crate::data::features::by_name as feature_by_name;
    pub use crate::feature::placement::PlacedFeature;
    pub use crate::feature::{ConfiguredFeature, FeatureCtx, VoxelSink};
}

/// Generate terrain + features for a chunk. Caller passes the world seed.
///
/// Terrain and feature placement both flow through the staged `ChunkGenerator`.
/// Features are placed via world-positional RNG over the chunk plus a margin
/// border, so trees cross chunk seams seamlessly.
///
/// The generator holds only immutable seed-derived state (noise samplers and
/// worldgen subsystems), which is expensive to build, so one-shot calls share
/// [`driver::ChunkGenerator::shared`] instead of rebuilding the pipeline per
/// chunk. Hot worker loops hold their own generator and call
/// [`generate_chunk_with`] directly.
pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    generate_chunk_with(&driver::ChunkGenerator::shared(seed), cx, cz)
}

/// Generate terrain + features with an already-built generator: the chunk's
/// sections from the one section pipeline, assembled (see
/// [`driver::ChunkGenerator::generate_chunk`]). Hot loops hold their own
/// generator and call this instead of [`generate_chunk`].
pub fn generate_chunk_with(generator: &driver::ChunkGenerator, cx: i32, cz: i32) -> Chunk {
    generator.generate_chunk(cx, cz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::block::Block;
    use petramond_world::chunk::{CHUNK_SX, CHUNK_SZ, SEA_LEVEL};

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
    fn cave_capable_section_summaries_are_conservative() {
        use super::noise::cave_field::CaveField;
        use petramond_world::section::SectionSummary;

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
    #[ignore = "slow sweep; `make test-worldgen` runs it"]
    fn per_section_generation_keeps_random_tick_count_exact() {
        use petramond_world::block::Block;
        use petramond_world::chunk::{SectionPos, CHUNK_SY, SECTION_SIZE};

        for &seed in &[1u32, 7, 42, 0x1234_5678] {
            let generator = driver::ChunkGenerator::new(seed);
            for cz in -3..=3 {
                for cx in -3..=3 {
                    let col = generator.generate_column_gen(cx, cz);
                    for cy in 0..(CHUNK_SY / SECTION_SIZE) as i32 {
                        let section = generator.generate_section(SectionPos::new(cx, cy, cz), &col);
                        let expected = section
                            .blocks_iter()
                            .filter(|&id| Block::from_id(id).has_random_tick())
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

    /// Frozen ponds carry bare sea ice: a snowy-biome column submerged under a
    /// waterline ice cap must NOT grow a snow layer above the ice (the
    /// vegetation pass never visits submerged columns). Seed 34's scanned
    /// coast holds thousands of such columns; assert on real ones so the rule
    /// cannot silently rot.
    #[test]
    fn frozen_ponds_carry_bare_sea_ice_without_a_snow_layer() {
        let seed = 34;
        let mut cases = 0;
        for &(cx, cz) in &[(6, -1), (7, -1), (8, -1)] {
            let chunk = generate_chunk(seed, cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let biome = petramond_world::biome::Biome::from_id(chunk.biome_at(x, z));
                    let snowy = matches!(
                        biome,
                        petramond_world::biome::Biome::SNOWY_PLAINS
                            | petramond_world::biome::Biome::SNOWY_TUNDRA
                            | petramond_world::biome::Biome::SNOWY_TAIGA
                    );
                    if snowy && chunk.block(x, SEA_LEVEL as usize, z) == Block::Ice {
                        cases += 1;
                        assert_eq!(
                            chunk.block(x, SEA_LEVEL as usize + 1, z),
                            Block::Air,
                            "snow layer on sea ice at ({cx},{cz}) local ({x},{z})"
                        );
                    }
                }
            }
        }
        assert!(cases > 0, "the scanned chunks must still hold frozen ponds");
    }

    #[test]

    #[ignore = "slow sweep; `make test-worldgen` runs it"]
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
                                    block != Block::Grass,
                                    "grass below sea level at chunk ({cx},{cz}) local ({x},{y},{z}) seed {seed:#x}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

}
