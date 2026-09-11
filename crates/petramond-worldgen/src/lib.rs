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

#![allow(clippy::too_many_arguments)]

pub mod audit;
pub mod biome;
pub mod colgen;
pub mod data;
pub mod density;
pub mod driver;
pub mod feature;
pub mod formula;
mod terrain_query;
pub use terrain_query::blocks_at as terrain_blocks_at;
pub use terrain_query::heights_at as terrain_heights_at;
pub use terrain_query::section_blocks as terrain_section_at;
pub mod graph;
pub mod hooks;
mod memo;
mod noise;
pub mod preview;
mod proto;
pub mod region;
pub mod rng;
mod section_memo;
pub mod spawn;
mod surface;

use petramond_world::chunk::Chunk;

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
        static CACHED: std::cell::RefCell<Option<((u32, u64), driver::ChunkGenerator)>> =
            const { std::cell::RefCell::new(None) };
    }
    // The cache key carries the installed worldgen-hook epoch alongside the
    // seed, so a session (re)installing mod hooks evicts generators that
    // captured the previous config. One atomic load; hookless processes see 0.
    let key = (seed, crate::hooks::installed_epoch());
    CACHED.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().map(|(k, _)| *k) != Some(key) {
            *slot = Some((key, driver::ChunkGenerator::new(seed)));
        }
        generate_chunk_with(&slot.as_ref().unwrap().1, cx, cz)
    })
}

/// Generate terrain + features with an already-built generator.
///
/// This preserves `generate_chunk` as the public one-shot API while allowing
/// hot worker loops to reuse the generator's immutable seed-derived state.
///
/// With mod worldgen hooks active, the chunk is assembled from the SAME
/// per-section path the cubic streamer runs (`generate_section`), so every
/// hook receives identical inputs per `(seed, section)` on both paths —
/// column/section parity is structural. With no hooks (the genparity pin),
/// the classic whole-chunk pipeline below runs untouched.
pub fn generate_chunk_with(generator: &driver::ChunkGenerator, cx: i32, cz: i32) -> Chunk {
    if generator.has_gen_hooks() {
        let mut chunk = generator.generate_chunk_via_sections(cx, cz);
        chunk.dirty = true;
        return chunk;
    }
    let mut chunk = generator.generate_surface(cx, cz);
    generator.carve_caves(&mut chunk);
    generator.place_underground(&mut chunk);
    generator.place_vegetation(&mut chunk);
    generator.place_features_runtime(&mut chunk);

    chunk.dirty = true;
    chunk
}

/// The underground biome owning each world position for `seed` — the same
/// climate partition used by cave lining and habitat decoration, so it answers
/// before any section exists. Purely positional: no loaded world, no order
/// dependence. This is the engine side of the mod ABI's `UndergroundBiomeAt`.
pub fn underground_biomes_at(seed: u32, positions: &[[i32; 3]]) -> Vec<u8> {
    let field = cave_field(seed);
    let clamped: Vec<[i32; 3]> = positions.iter().map(|p| clamp_query(*p)).collect();
    let mut out = Vec::new();
    field.underground_biome_at_batch(&clamped, &mut out);
    out
}

/// The conservative set of underground biome ids that can own a cell inside the
/// inclusive world box — the engine side of the mod ABI's
/// `UndergroundBiomesInBox`. An id it omits provably does not occur in the box,
/// so a mod whose content belongs to one biome can reject a whole dispatch on
/// it instead of asking cell by cell.
pub fn underground_biomes_in_box(seed: u32, lo: [i32; 3], hi: [i32; 3]) -> Vec<u8> {
    type Key = (u32, [usize; 2], [i32; 3], [i32; 3]);
    // Every section of a column asks about the same box, and neighbouring
    // columns about the same few.
    static BOXES: std::sync::LazyLock<memo::SharedMemo<Key, std::sync::Arc<[u8]>>> =
        std::sync::LazyLock::new(|| memo::SharedMemo::new(4096));
    let (lo, hi) = (clamp_query(lo), clamp_query(hi));
    let box_lo = std::array::from_fn(|a| lo[a].min(hi[a]));
    let box_hi = std::array::from_fn(|a| lo[a].max(hi[a]));
    let field = cave_field(seed);
    BOXES
        .get_or_compute_unlocked((seed, field.table_identities(), box_lo, box_hi), || {
            field
                .underground_biome_ids_in_box(box_lo, box_hi)
                .ids()
                .into()
        })
        .to_vec()
}

/// Is the generated terrain solid at each world position for `seed`? The
/// engine side of the mod ABI's `TerrainSolidAt`.
///
/// Solid means what the fill+carve stages leave behind: at or below the
/// column's density surface, and not cut away by a carver. Air and water are
/// both `false`; features (scatter, vegetation, trees, mod writes) are not
/// included, because they are not positional — they depend on a stage having
/// run.
///
/// This exists so a cross-section structure can make ONE acceptance decision
/// that every section agrees on, the way an engine feature's `is_anchored`
/// gate reads only the surface model and never chunk content. Reading the
/// dispatching section's own snapshot cannot do that: cells outside it are
/// unknown, so each section would answer differently for the same origin.
///
/// Surfaces come from the shared feature-window tile memo — the SAME values
/// the carve stage reads — so the answer cannot drift from the blocks a
/// section actually receives. Queries arrive in columns, so one tile is kept
/// hot rather than re-fetched per cell.
pub fn terrain_solid_at(seed: u32, positions: &[[i32; 3]]) -> Vec<bool> {
    terrain_samples(seed, positions)
        .into_iter()
        .map(|(_, _, solid)| solid)
        .collect()
}

pub use mod_api::TerrainSpace;

/// Distinguish dry clearance from aquifers and surface water without loading
/// neighbouring sections. Shares the terrain-solid query's surface/carve path.
pub fn terrain_space_at(seed: u32, positions: &[[i32; 3]]) -> Vec<TerrainSpace> {
    let samples = terrain_samples(seed, positions);
    let caves = cave_field(seed);
    let table = data::underground::table();
    let wet_candidates: Vec<_> = samples
        .iter()
        .filter_map(|(p, surface, solid)| {
            (!solid
                && p[1] <= *surface
                && table
                    .aquifer_y_span
                    .is_some_and(|(lo, hi)| (lo..=hi).contains(&p[1])))
            .then_some(*p)
        })
        .collect();
    let mut wet_biomes = underground_biomes_at(seed, &wet_candidates).into_iter();
    samples
        .into_iter()
        .map(|(pos, surface, solid)| {
            let field_space = caves.field_space_at(pos);
            if solid {
                return TerrainSpace::Solid;
            }
            if pos[1] > surface {
                return field_space.unwrap_or(if pos[1] <= petramond_world::chunk::SEA_LEVEL {
                    TerrainSpace::Water
                } else {
                    TerrainSpace::Air
                });
            }
            if table
                .aquifer_y_span
                .is_some_and(|(lo, hi)| (lo..=hi).contains(&pos[1]))
            {
                let biome = wet_biomes.next().expect("one biome per wet candidate");
                if table
                    .aquifer(biome)
                    .is_some_and(|aquifer| pos[1] <= aquifer.level)
                {
                    return field_space.unwrap_or(TerrainSpace::Water);
                }
            }
            field_space.unwrap_or(TerrainSpace::Air)
        })
        .collect()
}

fn terrain_samples(seed: u32, positions: &[[i32; 3]]) -> Vec<([i32; 3], i32, bool)> {
    use petramond_world::chunk::{SectionPos, SECTION_SIZE};
    const TILE: i32 = petramond_world::chunk::CHUNK_SX as i32;
    /// Positions inside one section from which filling and carving the whole
    /// section once — kept for its generation and every later probe — beats
    /// sampling them on their own lattice.
    const SECTION_MASK_MIN: usize = 128;
    let caves = cave_field(seed);
    let surface = surface_system(seed);
    let mut tile: Option<(i32, i32, Vec<i32>)> = None;
    // Pair each position with its column surface first (one tile fetch per
    // 16×16 run). Dense groups then read the memoized section terrain, and
    // the rest answer the carve question as one batch — one lattice per
    // spatial bucket instead of one per position.
    let mut queries: Vec<([i32; 3], i32)> = Vec::with_capacity(positions.len());
    // Positions the carve question cannot even apply to (above the column
    // surface, or below the carve floor).
    let mut no_carve = vec![false; positions.len()];
    // Probes arrive column by column, so a position's section is nearly always
    // the previous one's, and a call spans a few dozen sections at most.
    let mut groups: Vec<([i32; 3], Vec<u32>)> = Vec::new();
    let mut last = usize::MAX;
    for (i, p) in positions.iter().enumerate() {
        let [x, y, z] = clamp_query(*p);
        let (tcx, tcz) = (x.div_euclid(TILE), z.div_euclid(TILE));
        if !matches!(&tile, Some((cx, cz, _)) if *cx == tcx && *cz == tcz) {
            let (_, raw) = feature::cached_feature_region(
                &surface,
                &caves,
                seed,
                tcx * TILE,
                tcz * TILE,
                TILE as usize,
                TILE as usize,
            );
            tile = Some((tcx, tcz, raw));
        }
        let raw = &tile.as_ref().expect("tile just filled").2;
        let surf_y = raw[((z - tcz * TILE) * TILE + (x - tcx * TILE)) as usize];
        no_carve[i] = y > surf_y || (y < noise::settings::CAVE_MIN_Y && !caves.field_at_height(y));
        queries.push(([x, y, z], surf_y));
        let section = [tcx, y.div_euclid(SECTION_SIZE as i32), tcz];
        if last == usize::MAX || groups[last].0 != section {
            last = match groups.iter().position(|(key, _)| *key == section) {
                Some(k) => k,
                None => {
                    groups.push((section, Vec::new()));
                    groups.len() - 1
                }
            };
        }
        groups[last].1.push(i as u32);
    }
    let mut solid = vec![false; positions.len()];
    let mut sparse: Vec<([i32; 3], i32)> = Vec::new();
    let mut sparse_idx: Vec<u32> = Vec::new();
    for (sp, idx) in groups {
        let sp = SectionPos::new(sp[0], sp[1], sp[2]);
        let mask = if idx.len() >= SECTION_MASK_MIN {
            let (region, raw) = feature::cached_feature_region(
                &surface,
                &caves,
                seed,
                sp.cx * TILE,
                sp.cz * TILE,
                TILE as usize,
                TILE as usize,
            );
            let biomes: Vec<u8> = region.biomes.iter().map(|b| b.id()).collect();
            Some(section_memo::solid_mask(
                &surface, &caves, seed, sp, &biomes, &raw,
            ))
        } else {
            section_memo::solid_mask_if_memoized(&caves, seed, sp)
        };
        match mask {
            Some(mask) => {
                let (ox, oy, oz) = sp.origin_world();
                for &i in &idx {
                    let [x, y, z] = queries[i as usize].0;
                    let cell = petramond_world::chunk::section_idx(
                        (x - ox) as usize,
                        (y - oy) as usize,
                        (z - oz) as usize,
                    );
                    solid[i as usize] = mask[cell / 64] & (1 << (cell % 64)) != 0;
                }
            }
            None => {
                for &i in &idx {
                    sparse.push(queries[i as usize]);
                    sparse_idx.push(i);
                }
            }
        }
    }
    if !sparse.is_empty() {
        let mut carved = Vec::new();
        caves.cave_carved_batch(&sparse, &mut carved);
        for (k, &i) in sparse_idx.iter().enumerate() {
            let (p, surf_y) = queries[i as usize];
            solid[i as usize] = match caves.field_space_at(p) {
                Some(TerrainSpace::Solid) => true,
                Some(_) => false,
                None => p[1] <= surf_y && (no_carve[i as usize] || !carved[k]),
            };
        }
    }
    (0..positions.len())
        .map(|i| (queries[i].0, queries[i].1, solid[i]))
        .collect()
}

/// The final surface biome id at each world column for `seed` — the engine
/// side of the mod ABI's `SurfaceBiomeAt`.
///
/// The day-surface twin of [`terrain_solid_at`], and it exists for the same
/// reason: a worldgen hook's own column map covers only the dispatching
/// section, so it can neither carry a cross-section acceptance decision nor
/// answer anything about a NEIGHBOURING column. "Is there a river within N
/// blocks" — what tells a river bank apart from ordinary plains — is exactly
/// the second kind, and no column knows it about itself.
///
/// Read off the SAME world-anchored feature tile the feature stage reads, so
/// the answer cannot drift from the biome a section is actually dressed with.
pub fn surface_biome_at(seed: u32, columns: &[[i32; 2]]) -> Vec<u8> {
    const TILE: i32 = petramond_world::chunk::CHUNK_SX as i32;
    let caves = cave_field(seed);
    let surface = surface_system(seed);
    // Answered TILE BY TILE rather than in the caller's order: a batch of
    // neighbour probes around one column straddles a tile edge and would
    // otherwise re-take the memo lock on every other query.
    let mut order: Vec<u32> = (0..columns.len() as u32).collect();
    let key = |i: &u32| {
        let [x, _, z] = clamp_query([columns[*i as usize][0], 0, columns[*i as usize][1]]);
        (z.div_euclid(TILE), x.div_euclid(TILE))
    };
    order.sort_unstable_by_key(key);

    let mut out = vec![0u8; columns.len()];
    let mut tile: Option<((i32, i32), [petramond_world::biome::Biome; 256])> = None;
    for i in order {
        let [x, _, z] = clamp_query([columns[i as usize][0], 0, columns[i as usize][1]]);
        let at = (x.div_euclid(TILE), z.div_euclid(TILE));
        if !matches!(&tile, Some((k, _)) if *k == at) {
            tile = Some((
                at,
                feature::cached_tile_biomes(&surface, &caves, seed, at.0, at.1),
            ));
        }
        let biomes = &tile.as_ref().expect("tile just filled").1;
        out[i as usize] = biomes[((z - at.1 * TILE) * TILE + (x - at.0 * TILE)) as usize] as u8;
    }
    out
}

/// Guest-supplied coordinates are clamped before any positional query: the
/// cave lattice scales them by its step, so a position near the integer limits
/// would overflow that multiply, and no host call may be steered into
/// arithmetic UB by a mod. Y is clamped to the world column.
fn clamp_query(p: [i32; 3]) -> [i32; 3] {
    /// Leaves room for the lattice's `(cell + 1) * LATTICE_STEP` scaling.
    const HORIZONTAL_LIMIT: i32 = i32::MAX / 8;
    [
        p[0].clamp(-HORIZONTAL_LIMIT, HORIZONTAL_LIMIT),
        p[1].clamp(
            petramond_world::chunk::WORLD_MIN_Y,
            petramond_world::chunk::WORLD_MAX_Y - 1,
        ),
        p[2].clamp(-HORIZONTAL_LIMIT, HORIZONTAL_LIMIT),
    ]
}

/// Reuse the immutable cave sources and their bounded positional caches across
/// host queries. A seed change replaces the shared instance.
fn cave_field(seed: u32) -> std::sync::Arc<noise::cave_field::CaveField> {
    use std::sync::{Arc, Mutex};
    static SLOT: Mutex<Option<(u32, Arc<noise::cave_field::CaveField>)>> = Mutex::new(None);
    let mut slot = SLOT.lock().unwrap();
    match slot.as_ref() {
        Some((s, field)) if *s == seed => Arc::clone(field),
        _ => {
            let field = Arc::new(noise::cave_field::CaveField::new(seed));
            *slot = Some((seed, Arc::clone(&field)));
            field
        }
    }
}

/// The density graph behind the surface heights, memoized like [`cave_field`]:
/// building it is the expensive part of a generator, and the terrain query
/// would otherwise pay for it on every batch.
fn surface_system(seed: u32) -> std::sync::Arc<density::surface::SurfaceDensitySystem> {
    use std::sync::{Arc, Mutex};
    static SLOT: Mutex<Option<(u32, Arc<density::surface::SurfaceDensitySystem>)>> =
        Mutex::new(None);
    let mut slot = SLOT.lock().unwrap();
    match slot.as_ref() {
        Some((s, sys)) if *s == seed => Arc::clone(sys),
        _ => {
            let sys = Arc::new(density::surface::SurfaceDensitySystem::new(seed));
            *slot = Some((seed, Arc::clone(&sys)));
            sys
        }
    }
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;
    use petramond_world::block::Block;
    use petramond_world::chunk::{CHUNK_SX, CHUNK_SZ, SEA_LEVEL};

    /// The positional terrain query PROMISES the blocks a section will
    /// actually get. A mod uses it to decide, once, whether a structure that
    /// spans sections exists at all, so a drift between the query and the
    /// fill+carve pipeline puts mod content inside rock or floating in air —
    /// with nothing on either side to notice. Deep sections only: vegetation
    /// and trees add blocks the query deliberately does not model.
    #[test]
    fn the_terrain_query_matches_the_blocks_a_section_receives() {
        use petramond_world::chunk::SectionPos;
        use petramond_world::chunk::SECTION_SIZE;

        let seed = 0x0E58_1000;
        let gen = driver::ChunkGenerator::new(seed);
        let mut checked = 0usize;
        let mut open = 0usize;
        for (cx, cz) in [(0, 0), (3, -2), (-5, 7)] {
            let col = gen.generate_column_gen(cx, cz);
            for cy in [-4, -3, -2] {
                let section = gen.generate_section(SectionPos::new(cx, cy, cz), &col);
                let (ox, oy, oz) = section.origin_world();
                let mut probe = Vec::with_capacity(SECTION_SIZE.pow(3));
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            probe.push([ox + lx as i32, oy + ly as i32, oz + lz as i32]);
                        }
                    }
                }
                let solid = terrain_solid_at(seed, &probe);
                let mut i = 0;
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            let id = section.block_raw(lx, ly, lz);
                            let is_solid = id != Block::Air.id() && id != Block::Water.id();
                            assert_eq!(
                                solid[i],
                                is_solid,
                                "query says solid={} but section {:?} holds block {id} at {:?}",
                                solid[i],
                                (cx, cy, cz),
                                probe[i]
                            );
                            open += usize::from(!is_solid);
                            checked += 1;
                            i += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 0);
        assert!(
            open > 0,
            "no carved cell in the sample; the test is vacuous"
        );
    }

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
        use petramond_world::chunk::{SectionPos, CHUNK_SY, SECTION_SIZE};

        // Seed 31337's origin sits in a snowy region, so the snow-layer
        // placement (vegetation stage) is exercised across the seam too; seed
        // 34's chunks hold FROZEN PONDS (snowy-biome columns submerged under
        // waterline sea ice), the case where the chunk path must skip the
        // snow layer exactly like the section path skips the whole column.
        for &(seed, cx, cz) in &[
            (0x1234_5678u32, 0, 0),
            (0x1234_5678, 1, -1),
            (0x1234_5678, -3, 5),
            (0x1234_5678, 12, -7),
            (0x1234_5678, 4, -3),
            (31337, 0, 0),
            (31337, 2, 3),
            (31337, -1, -2),
            (34, 6, -1),
            (34, 7, -1),
            (34, 8, -1),
            // Forest groves straddle horizontal and vertical section boundaries.
            (786, 24, -24),
            (786, 25, -24),
            (786, 26, -25),
        ] {
            let generator = driver::ChunkGenerator::new(seed);
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
    /// per-section vegetation pass never visits submerged columns, so a layer
    /// here would be a chunk-vs-section parity break — the exact bug the
    /// slippery-top guard in `place_vegetation` exists to prevent). Seed 34's
    /// scanned coast holds thousands of such columns; assert on real ones so
    /// the guard cannot silently rot.
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
                        petramond_world::biome::Biome::SnowyPlains
                            | petramond_world::biome::Biome::SnowyTundra
                            | petramond_world::biome::Biome::SnowyTaiga
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
