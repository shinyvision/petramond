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

use std::cell::RefCell;
use std::sync::Arc;

use super::{CaveField, LATTICE_STEP};
use crate::data::underground::{ClimatePoint, IdSet, UndergroundBiomes};
use crate::memo::SharedMemo;

/// Leaf granularity in world blocks: two lattice cells per axis. Coarser reuses
/// better but snaps a query box further outward, and the whole value of the gate
/// is how tightly it bounds the caller's actual reach.
const BLOCK: i32 = 2 * LATTICE_STEP;
/// Leaves per grid axis: 32-block grids keep a first-touch computation short,
/// so workers needing the same grid at once wait only briefly for it.
const GRID: i32 = 4;
const GRID_BLOCKS: i32 = GRID * BLOCK;
const LEAVES: usize = (GRID * GRID * GRID) as usize;

/// Everything a grid's leaf sets are a function of. The PARTITION TABLE belongs
/// in here as much as the seed does: the set is a fact about a table's bands,
/// and the seed alone does not name one — a second table can be interned in the
/// same process (a test bench, a re-layered pack) and would otherwise read the
/// first one's answers out of these slots. The table is `&'static`, so its
/// address is its identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    seed: u32,
    table: usize,
    pos: [i32; 3],
}

struct Grid {
    leaves: Box<[IdSet; LEAVES]>,
    /// The union of every leaf, for a box that covers the whole grid.
    all: IdSet,
}

static GRIDS: std::sync::LazyLock<SharedMemo<Key, Arc<Grid>>> =
    std::sync::LazyLock::new(|| SharedMemo::new(8192));

// A query touches at most eight grids and the next section's query the same
// ones, so a small per-thread copy fronts the shared slots' locks.
const LOCAL_ENTRIES: usize = 256;
type LocalEntry = Option<(Key, Arc<Grid>)>;
thread_local! {
    static LOCAL: RefCell<Vec<LocalEntry>> = RefCell::new(vec![None; LOCAL_ENTRIES]);
}

impl CaveField {
    /// The conservative set of underground biome ids that can own a cell inside
    /// the inclusive world box `lo..=hi`, snapped outward to the leaf grid.
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

    fn grid(&self, gp: [i32; 3]) -> Arc<Grid> {
        let key = Key {
            seed: self.seed,
            table: std::ptr::from_ref(self.underground) as usize,
            pos: gp,
        };
        let hash = (gp[0] as u32 as u64)
            ^ (gp[1] as u32 as u64).rotate_left(21)
            ^ (gp[2] as u32 as u64).rotate_left(42)
            ^ self.seed as u64;
        let slot = (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 56) as usize;
        if let Some(hit) = LOCAL.with(|cache| match &cache.borrow()[slot] {
            Some((saved, grid)) if *saved == key => Some(Arc::clone(grid)),
            _ => None,
        }) {
            return hit;
        }
        let grid = GRIDS.get_or_compute_unlocked(key, || Arc::new(self.compute_grid(gp)));
        LOCAL.with(|cache| cache.borrow_mut()[slot] = Some((key, Arc::clone(&grid))));
        grid
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

    /// One leaf on its own, the reference the grid must agree with.
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

/// The ids that can own a cell of the leaf whose bottom is `y_lo`, from its
/// nine climate columns (row-major, west to east then north to south).
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
