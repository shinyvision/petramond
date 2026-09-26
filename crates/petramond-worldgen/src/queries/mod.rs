//! The positional queries: what generation WILL produce at a position,
//! answered without generating or loading the section — heights, blocks,
//! terrain occupancy and surface / underground biomes. The engine side of the
//! mod ABI's positional host calls, and what spawn and pack checks ask.
//!
//! Every answer reads the same memos the section pipeline fills, so a query
//! can never drift from the blocks a section actually receives.

use mod_api::TerrainSpace;

use crate::{cache, density, driver, feature, noise, section_memo};

mod terrain;
pub use terrain::{blocks_at, heights_at, section_blocks};

/// The underground biome owning each world position for `seed` — the same
/// climate partition used by cave lining and habitat decoration, so it answers
/// before any section exists. Purely positional: no loaded world, no order
/// dependence. This is the engine side of the mod ABI's `UndergroundBiomeAt`.
pub fn underground_biomes_at(seed: u32, positions: &[[i32; 3]]) -> Vec<u8> {
    let generator = driver::ChunkGenerator::shared(seed);
    let (_, field) = generator.sources();
    let clamped: Vec<[i32; 3]> = positions.iter().map(|p| clamp_query(*p)).collect();
    let mut out = Vec::new();
    field.underground_biome_at_batch(&clamped, &mut out);
    out
}

/// The key of a memoized [`underground_biomes_in_box`] answer: the cave
/// field's context and the normalized box.
pub(crate) type UndergroundBoxKey = (cache::GenContext, [i32; 3], [i32; 3]);

/// The conservative set of underground biome ids that can own a cell inside the
/// inclusive world box — the engine side of the mod ABI's
/// `UndergroundBiomesInBox`. An id it omits provably does not occur in the box,
/// so a mod whose content belongs to one biome can reject a whole dispatch on
/// it instead of asking cell by cell.
pub fn underground_biomes_in_box(seed: u32, lo: [i32; 3], hi: [i32; 3]) -> Vec<u8> {
    // Every section of a column asks about the same box, and neighbouring
    // columns about the same few.
    let (lo, hi) = (clamp_query(lo), clamp_query(hi));
    let box_lo = std::array::from_fn(|a| lo[a].min(hi[a]));
    let box_hi = std::array::from_fn(|a| lo[a].max(hi[a]));
    let generator = driver::ChunkGenerator::shared(seed);
    let (_, field) = generator.sources();
    let key = (field.context(), box_lo, box_hi);
    field
        .caches()
        .terrain
        .underground_boxes
        .get_or_compute_unlocked(key, || {
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
/// column's density surface, and not cut away by a carver. Air and fluids are
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
        .map(|(_, _, space)| space == TerrainSpace::Solid)
        .collect()
}


/// [`terrain_solid_at`] telling air from fluid, without loading neighbouring
/// sections: a cell holding the sea, an aquifer, a pool or a fall is neither
/// ground to stand on nor room to grow into.
pub fn terrain_space_at(seed: u32, positions: &[[i32; 3]]) -> Vec<TerrainSpace> {
    terrain_samples(seed, positions)
        .into_iter()
        .map(|(_, _, space)| space)
        .collect()
}

/// Each position's `(clamped position, column surface, terrain occupancy)`.
fn terrain_samples(seed: u32, positions: &[[i32; 3]]) -> Vec<([i32; 3], i32, TerrainSpace)> {
    let generator = driver::ChunkGenerator::shared(seed);
    let (surface, caves) = generator.sources();
    terrain_samples_in(caves, surface, positions)
}

/// [`terrain_samples`] over explicit generation sources.
fn terrain_samples_in(
    caves: &noise::cave_field::CaveField,
    surface: &density::surface::SurfaceDensitySystem,
    positions: &[[i32; 3]],
) -> Vec<([i32; 3], i32, TerrainSpace)> {
    use petramond_world::chunk::{SectionPos, SECTION_SIZE};
    const TILE: i32 = petramond_world::chunk::CHUNK_SX as i32;
    /// Positions inside one section from which filling and carving the whole
    /// section once — kept for its generation and every later probe — beats
    /// sampling them on their own lattice.
    const SECTION_MASK_MIN: usize = 128;
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
                surface,
                caves,
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
    let mut space = vec![TerrainSpace::Air; positions.len()];
    let mut sparse: Vec<([i32; 3], i32)> = Vec::new();
    let mut sparse_idx: Vec<u32> = Vec::new();
    for (sp, idx) in groups {
        let sp = SectionPos::new(sp[0], sp[1], sp[2]);
        let mask = if idx.len() >= SECTION_MASK_MIN {
            Some(section_memo::space_mask(surface, caves, sp))
        } else {
            section_memo::space_mask_if_memoized(caves, sp)
        };
        match mask {
            Some(mask) => {
                let (ox, oy, oz) = sp.origin_world();
                for &i in &idx {
                    let [x, y, z] = queries[i as usize].0;
                    space[i as usize] = mask.at(petramond_world::chunk::section_idx(
                        (x - ox) as usize,
                        (y - oy) as usize,
                        (z - oz) as usize,
                    ));
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
        let mut fills = Vec::new();
        caves.cave_fill_batch(&sparse, &mut fills);
        for (k, &i) in sparse_idx.iter().enumerate() {
            let (p, surf_y) = queries[i as usize];
            space[i as usize] = match caves.field_space_at(p) {
                Some(field) => field,
                // Above the column's surface the terrain fill, not the carve,
                // decides: open sky, or the ocean standing in it.
                None if p[1] > surf_y => {
                    if p[1] <= petramond_world::chunk::SEA_LEVEL {
                        TerrainSpace::Fluid
                    } else {
                        TerrainSpace::Air
                    }
                }
                None if no_carve[i as usize] => TerrainSpace::Solid,
                None => match fills[k] {
                    Some(block) => section_memo::space_of(block),
                    None => TerrainSpace::Solid,
                },
            };
        }
        // The falls the section stamp and the memoized mask apply, from the
        // same per-chunk claims, so a sparse answer matches a cached one.
        if let Some(top) = caves.falls_top() {
            let mut chunk = None;
            for &i in &sparse_idx {
                let p = queries[i as usize].0;
                if p[1] > top {
                    continue;
                }
                let at = [p[0].div_euclid(TILE), p[2].div_euclid(TILE)];
                if !matches!(&chunk, Some((c, _)) if *c == at) {
                    chunk = Some((at, caves.chunk_falls(at[0], at[1])));
                }
                let (_, falls) = chunk.as_ref().expect("chunk just fetched");
                space[i as usize] = falls.space_at(p, space[i as usize]);
            }
        }
    }
    (0..positions.len())
        .map(|i| (queries[i].0, queries[i].1, space[i]))
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
    let generator = driver::ChunkGenerator::shared(seed);
    let (surface, caves) = generator.sources();
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
                feature::cached_tile_biomes(&surface, &caves, at.0, at.1),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data;

    /// The positional terrain query PROMISES the blocks a section will
    /// actually get. A mod uses it to decide, once, whether a structure that
    /// spans sections exists at all, so a drift between the query and the
    /// fill+carve pipeline puts mod content inside rock, floating in air, or
    /// standing in a fluid. Deep sections only: vegetation and trees add
    /// blocks the query deliberately does not model. The shipped habitats
    /// carry a fall row every column rolls, and the sample includes a chunk
    /// holding a fall, whose cells are asked sparsely before their sections'
    /// terrain is memoized and again after: the answer must not depend on
    /// what is cached.
    #[test]
    fn the_terrain_query_matches_the_blocks_a_section_receives() {
        use petramond_world::chunk::SectionPos;
        use petramond_world::chunk::SECTION_SIZE;

        // The synthetic table is a context of its own: every memo key names
        // its content, so it shares no entry with the shipped table's worlds.
        let seed = 0x0E58_1001;
        const ALWAYS: &str = r#"{"fluid_falls":[{"fluid_fall":"test:always","fluid":"petramond:lava",
            "chance":1.0,"y":[-38,-11],"min_surface":45}]}"#;
        let shipped = data::underground::shipped_layer();
        let table = data::underground::synthetic_table(&[&shipped, ALWAYS]);
        let gen = driver::ChunkGenerator::with_caves(
            seed,
            noise::cave_field::CaveField::with_table(seed, table),
        );
        let (surface, caves) = gen.sources();
        let space_at = |positions: &[[i32; 3]]| -> Vec<TerrainSpace> {
            terrain_samples_in(caves, surface, positions)
                .into_iter()
                .map(|(_, _, space)| space)
                .collect()
        };
        let mut near: Vec<(i32, i32)> =
            (-2..2).flat_map(|z| (-2..2).map(move |x| (x, z))).collect();
        near.sort_by_key(|&(x, z)| x * x + z * z);
        let (fall_chunk, mut fall_cells) = near
            .into_iter()
            .find_map(|(cx, cz)| {
                let mut cells = Vec::new();
                caves
                    .chunk_falls(cx, cz)
                    .cells([i32::MIN; 3], [i32::MAX; 3], |c| cells.push(c.pos));
                (!cells.is_empty()).then_some(((cx, cz), cells))
            })
            .expect("a fall where every column rolls one");
        // Few enough per section that every one is answered sparsely.
        fall_cells.truncate(100);
        let cold = space_at(&fall_cells);
        let fall_cys: std::collections::BTreeSet<i32> =
            fall_cells.iter().map(|p| p[1].div_euclid(16)).collect();

        let mut checked = 0usize;
        let mut open = 0usize;
        let mut fluid = 0usize;
        let sample = [(0, 0), (3, -2), (-5, 7), fall_chunk];
        for (cx, cz) in sample {
            let col = gen.generate_column_gen(cx, cz);
            let cys: Vec<i32> = if (cx, cz) == fall_chunk {
                fall_cys.iter().copied().collect()
            } else {
                vec![-4, -3, -2]
            };
            for cy in cys {
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
                let space = space_at(&probe);
                let mut i = 0;
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            let id = section.block_raw(lx, ly, lz);
                            let held = section_memo::space_of(id);
                            assert_eq!(
                                space[i],
                                held,
                                "query says {:?} but section {:?} holds block {id} at {:?}",
                                space[i],
                                (cx, cy, cz),
                                probe[i]
                            );
                            fluid += usize::from(held == TerrainSpace::Fluid);
                            open += usize::from(held == TerrainSpace::Air);
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
        assert!(
            fluid > 0,
            "no fluid cell in the sample, so the half of this that keeps \
             content out of a fluid proves nothing — widen the sample"
        );
        let warm = space_at(&fall_cells);
        assert_eq!(cold, warm, "a fall's cells answer differently once cached");
        assert!(
            warm.contains(&TerrainSpace::Fluid),
            "no fall cell holds fluid"
        );
    }

    /// The height and block queries read the merged surface memos (the cave
    /// field's density surfaces, the shared surface tiles); they must answer
    /// exactly the surfaces and biomes the terrain fill uses.
    #[test]
    fn the_terrain_queries_read_the_fill_inputs() {
        let seed = 0x4EA7_0001;
        let surface = density::surface::SurfaceDensitySystem::new(seed);
        let columns: Vec<[i32; 2]> = (-4..4)
            .flat_map(|x| (-3..3).map(move |z| [x * 37 + 5, z * 29 - 11]))
            .collect();
        let expected: Vec<i32> = columns
            .iter()
            .map(|&[x, z]| surface.surface_heights(x, z, 1, 1)[0])
            .collect();
        assert_eq!(heights_at(seed, &columns), expected);

        let generator = driver::ChunkGenerator::shared(seed);
        let (surface, caves) = generator.sources();
        for (cx, cz) in [(0, 0), (-3, 2)] {
            let region = surface.region(cx * 16, cz * 16, 16, 16);
            let (raw, biomes) = feature::cached_tile_raw(surface, caves, cx, cz);
            assert_eq!(raw.as_slice(), region.surf.as_slice());
            assert_eq!(biomes.as_slice(), region.biomes.as_slice());
        }
    }
}
