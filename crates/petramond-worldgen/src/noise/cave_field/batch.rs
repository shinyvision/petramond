//! Batched cave point queries.
//!
//! One point query builds a one-voxel lattice: 8 world-anchored corners for a single answer. The
//! mod ABI's positional queries (`TerrainSolidAt`, `UndergroundBiomeAt`) show up hundreds at a time
//! and tend to cluster, so doing that per position samples the same corners again and again.
//!
//! We split a batch recursively instead. A box shares one lattice if that costs no more corners
//! than the naive `8 × positions`; if not, it's cut at its longest axis and each half tries again.
//! Dense batches end up as one lattice, and scattered points cost the same as before.
//!
//! Output is identical. Corners sit on `LATTICE_STEP`, so a shared lattice has the same corner
//! values a one-voxel lattice would. Dropping a chamber that's zero everywhere in the box is the
//! one box-dependent filter, and it's value-neutral by contract.

use petramond_world::block::Block;

use super::query_cache::QueryMemo;
use super::{CaveCut, CaveField, Col, Fields, CAVE_MIN_Y, CAVE_SURFACE_BUFFER, LATTICE_STEP};

struct Carve {
    idx: u32,
    gate: bool,
    interior: bool,
}

impl CaveField {
    pub fn cave_carved_batch(&self, queries: &[([i32; 3], i32)], out: &mut Vec<bool>) {
        self.cut_batch(
            queries,
            false,
            &self.memos().carved,
            false,
            CaveCut::is_open,
            out,
        );
    }

    pub fn cave_fill_batch(&self, queries: &[([i32; 3], i32)], out: &mut Vec<Option<u16>>) {
        let fill = |cut| match cut {
            CaveCut::Air => Some(Block::Air.id()),
            CaveCut::Fill(block) if !Block::from_id(block).is_solid() => Some(block),
            _ => None,
        };
        self.cut_batch(queries, true, &self.memos().filled, None, fill, out);
    }

    fn cut_batch<T: Clone>(
        &self,
        queries: &[([i32; 3], i32)],
        fluids: bool,
        memo: &QueryMemo<T>,
        rock: T,
        answer: impl Fn(CaveCut) -> T,
        out: &mut Vec<T>,
    ) {
        out.clear();
        out.resize(queries.len(), rock);

        let mut work: Vec<Carve> = Vec::new();
        for (i, ([_, y, _], surf_y)) in queries.iter().enumerate() {
            let (y, surf_y) = (*y, *surf_y);
            if y > surf_y + self.excavations.surface_offset {
                continue;
            }
            let gate = Self::entrance_allowed(y, surf_y);
            let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
            if !gate && !interior && !self.field_at_height(y) {
                continue;
            }
            work.push(Carve {
                idx: i as u32,
                gate,
                interior,
            });
        }

        if work.is_empty() {
            return;
        }
        self.cache_carve_queries(memo, queries, out, |out| {
            subdivide(
                &mut work,
                |w| queries[w.idx as usize].0,
                &mut |group, lo, hi| {
                    let any_interior = group.iter().any(|w| w.interior);
                    let fields = Fields {
                        carve: true,
                        interior: any_interior,
                        biome: false,
                        excavations: true,
                        positioned: true,
                        fluids,
                    };
                    let lat = self
                        .build_lattice_filtered(lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], fields);
                    group.sort_unstable_by_key(|w| {
                        let [x, y, z] = queries[w.idx as usize].0;
                        (x, z, y)
                    });
                    let mut cursor: Option<([i32; 2], Col)> = None;
                    for w in group.iter() {
                        let [x, y, z] = queries[w.idx as usize].0;
                        let c = match &mut cursor {
                            Some((at, c)) if *at == [x, z] => c,
                            slot => &mut slot.insert(([x, z], Col::new(&lat, x, z))).1,
                        };
                        out[w.idx as usize] = answer(self.cut_from_col(c, y, w.gate, w.interior));
                    }
                },
            );
        });
    }

    pub fn underground_biome_at_batch(&self, positions: &[[i32; 3]], out: &mut Vec<u8>) {
        out.clear();
        out.resize(positions.len(), 0);
        const FIELDS: Fields = Fields {
            carve: false,
            interior: false,
            biome: true,
            excavations: true,
            positioned: true,
            fluids: false,
        };
        let mut order: Vec<u32> = (0..positions.len() as u32).collect();
        subdivide(
            &mut order,
            |i| positions[*i as usize],
            &mut |group, lo, hi| {
                let lat =
                    self.build_lattice_filtered(lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], FIELDS);
                group.sort_unstable_by_key(|i| {
                    let [x, y, z] = positions[*i as usize];
                    (x, z, y)
                });
                let mut cursor: Option<([i32; 2], Col)> = None;
                for &i in group.iter() {
                    let [x, y, z] = positions[i as usize];
                    let c = match &mut cursor {
                        Some((at, c)) if *at == [x, z] => c,
                        slot => &mut slot.insert(([x, z], Col::new(&lat, x, z))).1,
                    };
                    out[i as usize] = self.biome_id_col(c, y);
                }
            },
        );
    }
}

fn corner_count(lo: [i32; 3], hi: [i32; 3]) -> u64 {
    (0..3)
        .map(|a| (hi[a].div_euclid(LATTICE_STEP) - lo[a].div_euclid(LATTICE_STEP) + 2) as u64)
        .product()
}

fn subdivide<T>(
    items: &mut [T],
    pos: impl Fn(&T) -> [i32; 3] + Copy,
    eval: &mut impl FnMut(&mut [T], [i32; 3], [i32; 3]),
) {
    if items.is_empty() {
        return;
    }
    let (lo, hi) = extents(items.iter().map(pos));
    if corner_count(lo, hi) <= 8 * items.len() as u64 {
        eval(items, lo, hi);
        return;
    }
    let axis = (0..3).max_by_key(|&a| hi[a] - lo[a]).expect("three axes");
    let mid = (lo[axis] + (hi[axis] - lo[axis]) / 2).div_euclid(LATTICE_STEP) * LATTICE_STEP;
    let n = partition(items, |t| pos(t)[axis] < mid);
    if n == 0 || n == items.len() {
        eval(items, lo, hi);
        return;
    }
    let (a, b) = items.split_at_mut(n);
    subdivide(a, pos, eval);
    subdivide(b, pos, eval);
}

fn partition<T>(items: &mut [T], pred: impl Fn(&T) -> bool) -> usize {
    let mut n = 0;
    for i in 0..items.len() {
        if pred(&items[i]) {
            items.swap(i, n);
            n += 1;
        }
    }
    n
}

fn extents(points: impl Iterator<Item = [i32; 3]>) -> ([i32; 3], [i32; 3]) {
    let mut lo = [i32::MAX; 3];
    let mut hi = [i32::MIN; 3];
    for p in points {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    (lo, hi)
}
