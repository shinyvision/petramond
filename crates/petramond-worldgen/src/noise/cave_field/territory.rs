//! "Which underground biomes can be in this box?" — the bounded form of the
//! per-cell partition query.
//!
//! A mod placing content that belongs to one underground biome otherwise has to
//! ask cell by cell, and pays the full biome field for every candidate in every
//! section it is dispatched for — including the overwhelming majority of
//! sections that hold none of its territory at all. This answers the question
//! once for a whole box, conservatively: an id it omits provably does not own a
//! cell in the box, so a caller may reject on that and nothing else.
//!
//! Cost comes from bounding rather than evaluating. Trilinear values are convex
//! combinations of the eight cell corners, so a lattice cell's field WINDOW is
//! its corner min/max, and the table answers which rows can claim that window at
//! that depth. Leaves are world-anchored [`BLOCK`]³ cubes; they are computed a
//! whole [`GRID`]³ of them at a time and memoized per grid, because a query box
//! from a section reads a few hundred leaves that neighbouring sections' boxes
//! read again — a dense grid answers each with one memory read where a
//! hierarchy of separately locked cubes paid a lock per node.

use std::sync::Arc;

use super::{CaveField, LATTICE_STEP};
use crate::cache::local::{self, LocalTable};
use crate::data::underground::{ClimatePoint, IdSet, UndergroundBiomes};

const BLOCK: i32 = 2 * LATTICE_STEP;
const GRID: i32 = 4;
const GRID_BLOCKS: i32 = GRID * BLOCK;
const LEAVES: usize = (GRID * GRID * GRID) as usize;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Key {
    context: crate::cache::GenContext,
    pos: [i32; 3],
}

pub(super) struct Grid {
    leaves: Box<[IdSet; LEAVES]>,
    all: IdSet,
}

thread_local! {
    static LOCAL: LocalTable<Key, Arc<Grid>> = LocalTable::new(&local::CAVE_TERRITORY);
}

impl CaveField {
    pub fn underground_biome_ids_in_box(&self, lo: [i32; 3], hi: [i32; 3]) -> IdSet {
        let mut out = IdSet::default();
        super::volumes::claims::include(self, lo, hi, &mut out);
        let lo = lo.map(|v| v.div_euclid(BLOCK));
        let hi = hi.map(|v| v.div_euclid(BLOCK));
        for gy in lo[1].div_euclid(GRID)..=hi[1].div_euclid(GRID) {
            for gz in lo[2].div_euclid(GRID)..=hi[2].div_euclid(GRID) {
                for gx in lo[0].div_euclid(GRID)..=hi[0].div_euclid(GRID) {
                    let grid = self.grid([gx, gy, gz]);
                    let origin = [gx * GRID, gy * GRID, gz * GRID];
                    let a: [i32; 3] = std::array::from_fn(|i| (lo[i] - origin[i]).max(0));
                    let b: [i32; 3] = std::array::from_fn(|i| (hi[i] - origin[i]).min(GRID - 1));
                    if a == [0; 3] && b == [GRID - 1; 3] {
                        out.union(&grid.all);
                        continue;
                    }
                    for ly in a[1]..=b[1] {
                        for lz in a[2]..=b[2] {
                            for lx in a[0]..=b[0] {
                                out.union(&grid.leaves[((ly * GRID + lz) * GRID + lx) as usize]);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// [`underground_biome_ids_in_box`](Self::underground_biome_ids_in_box) for one id, a bit per
    /// leaf of the box (the excavation claims over the box admit every leaf, as there).
    pub fn underground_biome_leaf_mask(
        &self,
        lo: [i32; 3],
        hi: [i32; 3],
        id: u8,
    ) -> mod_api::LeafMask {
        let mut claimed = IdSet::default();
        super::volumes::claims::include(self, lo, hi, &mut claimed);
        let mut mask = mod_api::LeafMask::over(BLOCK, lo, hi);
        let (min, size) = (mask.min, mask.size);
        let max = [0, 1, 2].map(|a| min[a] + size[a] - 1);
        let bit = |l: [i32; 3]| {
            (((l[1] - min[1]) * size[2] + (l[2] - min[2])) * size[0] + (l[0] - min[0])) as usize
        };
        if claimed.contains(id) {
            for l1 in min[1]..=max[1] {
                for l2 in min[2]..=max[2] {
                    for l0 in min[0]..=max[0] {
                        mask.set(bit([l0, l1, l2]));
                    }
                }
            }
            return mask;
        }
        for gy in min[1].div_euclid(GRID)..=max[1].div_euclid(GRID) {
            for gz in min[2].div_euclid(GRID)..=max[2].div_euclid(GRID) {
                for gx in min[0].div_euclid(GRID)..=max[0].div_euclid(GRID) {
                    let grid = self.grid([gx, gy, gz]);
                    if !grid.all.contains(id) {
                        continue;
                    }
                    let origin = [gx * GRID, gy * GRID, gz * GRID];
                    for ly in (origin[1].max(min[1]))..=(origin[1] + GRID - 1).min(max[1]) {
                        for lz in (origin[2].max(min[2]))..=(origin[2] + GRID - 1).min(max[2]) {
                            for lx in (origin[0].max(min[0]))..=(origin[0] + GRID - 1).min(max[0]) {
                                let leaf = (((ly - origin[1]) * GRID + (lz - origin[2])) * GRID
                                    + (lx - origin[0]))
                                    as usize;
                                if grid.leaves[leaf].contains(id) {
                                    mask.set(bit([lx, ly, lz]));
                                }
                            }
                        }
                    }
                }
            }
        }
        mask
    }

    /// Per lattice cell (`origin` and `dims` in lattice steps, cells indexed
    /// `(y * dims_z + z) * dims_x + x`): whether no climate row can win there.
    /// A cell lies inside one leaf, and a leaf's set is conservative for every
    /// point of it, the same guarantee [`Self::underground_biome_ids_in_box`] gives.
    pub(super) fn ordinary_cells(&self, origin: [i32; 3], dims: [usize; 3]) -> Vec<bool> {
        const CELLS_PER_LEAF: i32 = BLOCK / LATTICE_STEP;
        let mut out = Vec::with_capacity(dims.iter().product());
        let mut last: Option<([i32; 3], Arc<Grid>)> = None;
        for cy in 0..dims[1] as i32 {
            for cz in 0..dims[2] as i32 {
                for cx in 0..dims[0] as i32 {
                    let leaf = [origin[0] + cx, origin[1] + cy, origin[2] + cz]
                        .map(|v| v.div_euclid(CELLS_PER_LEAF));
                    let gp = leaf.map(|v| v.div_euclid(GRID));
                    let grid = match &last {
                        Some((at, grid)) if *at == gp => grid,
                        _ => &last.insert((gp, self.grid(gp))).1,
                    };
                    let [lx, ly, lz] = leaf.map(|v| v.rem_euclid(GRID));
                    out.push(
                        grid.leaves[((ly * GRID + lz) * GRID + lx) as usize].is_ordinary_only(),
                    );
                }
            }
        }
        out
    }

    fn grid(&self, gp: [i32; 3]) -> Arc<Grid> {
        let key = Key {
            context: self.context().without_excavations(),
            pos: gp,
        };
        let hash = (gp[0] as u32 as u64)
            ^ (gp[1] as u32 as u64).rotate_left(21)
            ^ (gp[2] as u32 as u64).rotate_left(42)
            ^ self.seed as u64;
        LOCAL.with(|table| {
            table.get_or_insert_with(local::spread(hash), key, || {
                self.memos()
                    .territory
                    .get_or_compute_unlocked(key, || Arc::new(self.compute_grid(gp)))
            })
        })
    }

    /// Every leaf of one grid from its shared climate columns: a leaf's nine
    /// columns are its neighbours' too, so the grid samples `(2·GRID+1)²`
    /// columns once instead of nine per leaf.
    fn compute_grid(&self, gp: [i32; 3]) -> Grid {
        let lo = gp.map(|v| v * GRID_BLOCKS);
        let n = (2 * GRID + 1) as usize;
        let columns: Vec<ClimatePoint> = (0..n * n)
            .map(|i| {
                self.climate_column(
                    lo[0] + (i % n) as i32 * LATTICE_STEP,
                    lo[2] + (i / n) as i32 * LATTICE_STEP,
                )
            })
            .collect();
        let mut leaves = Box::new([IdSet::default(); LEAVES]);
        let mut all = IdSet::default();
        for lz in 0..GRID {
            for lx in 0..GRID {
                let nine: [ClimatePoint; 9] = std::array::from_fn(|i| {
                    columns[(lz * 2 + (i / 3) as i32) as usize * n
                        + (lx * 2 + (i % 3) as i32) as usize]
                });
                for ly in 0..GRID {
                    let y = lo[1] + ly * BLOCK;
                    let mut ids = leaf_ids(self.underground, &nine, y);
                    let start = [lo[0] + lx * BLOCK, y, lo[2] + lz * BLOCK];
                    self.include_region_biomes(start, start.map(|v| v + BLOCK - 1), &mut ids);
                    leaves[((ly * GRID + lz) * GRID + lx) as usize] = ids;
                    all.union(&ids);
                }
            }
        }
        Grid { leaves, all }
    }

    #[cfg(test)]
    fn compute_block_ids(&self, sp: [i32; 3]) -> IdSet {
        let lo = [sp[0] * BLOCK, sp[1] * BLOCK, sp[2] * BLOCK];
        let columns: [_; 9] = std::array::from_fn(|i| {
            self.climate_column(
                lo[0] + (i % 3) as i32 * LATTICE_STEP,
                lo[2] + (i / 3) as i32 * LATTICE_STEP,
            )
        });
        let mut ids = leaf_ids(self.underground, &columns, lo[1]);
        self.include_region_biomes(lo, lo.map(|v| v + BLOCK - 1), &mut ids);
        ids
    }
}

fn leaf_ids(table: &UndergroundBiomes, columns: &[ClimatePoint; 9], y_lo: i32) -> IdSet {
    let mut out = IdSet::default();
    for cz in 0..2 {
        for cx in 0..2 {
            let mut climate = [[f64::INFINITY, f64::NEG_INFINITY]; 6];
            for d in 0..4usize {
                let point = columns[(cz + (d >> 1)) * 3 + cx + (d & 1)];
                for (range, value) in climate.iter_mut().zip(point) {
                    range[0] = range[0].min(value);
                    range[1] = range[1].max(value);
                }
            }
            let heights = climate[5];
            for cy in 0..2 {
                let wy = y_lo + cy * LATTICE_STEP;
                climate[5] = [
                    (heights[0] - (wy + LATTICE_STEP) as f64) / 128.0,
                    (heights[1] - wy as f64) / 128.0,
                ];
                table.ids_in((wy, wy + LATTICE_STEP - 1), climate, &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_query_is_the_exact_union_of_covered_leaf_blocks() {
        let table = crate::data::underground::test_table(&[r#"{"underground_biomes":[
            {"underground_biome":"test:shallow","y":[0,63],"climate":{"depth":[-2.0,2.0]}},
            {"underground_biome":"test:deep","y":[-48,-1],"climate":{"depth":[-2.0,2.0]}}
        ]}"#]);
        let field =
            CaveField::with_tables(91, table, crate::data::excavations::test_table(&[], table));
        let mut seen = IdSet::default();
        for (lo, hi) in [
            ([-71_i32, -25, -5], [7_i32, 16, 68]),
            ([0, -64, 0], [63, -1, 63]),
            ([-1, -1, -1], [0, 0, 0]),
            ([-64, -64, -64], [63, 63, 63]),
        ] {
            let mut expected = IdSet::default();
            for y in lo[1].div_euclid(BLOCK)..=hi[1].div_euclid(BLOCK) {
                for z in lo[2].div_euclid(BLOCK)..=hi[2].div_euclid(BLOCK) {
                    for x in lo[0].div_euclid(BLOCK)..=hi[0].div_euclid(BLOCK) {
                        expected.union(&field.compute_block_ids([x, y, z]));
                    }
                }
            }
            let got = field.underground_biome_ids_in_box(lo, hi);
            seen.union(&got);
            for id in 0..=255 {
                assert_eq!(got.contains(id), expected.contains(id));
            }
        }
        assert!(seen.contains(table.id("test:shallow").unwrap()));
        assert!(seen.contains(table.id("test:deep").unwrap()));
    }
}
