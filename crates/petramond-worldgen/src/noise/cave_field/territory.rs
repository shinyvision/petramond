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
//! that depth. The answer is then memoized per world-anchored [`BLOCK`]³ cube:
//! query boxes from neighbouring sections overlap heavily, and a cube's set is a
//! pure function of `(seed, cube)`.

use std::sync::Mutex;

use super::{CaveField, Fields, LATTICE_STEP};
use crate::data::underground::IdSet;

/// Memo granularity in world blocks: two lattice cells per axis. Coarser reuses
/// better but snaps a query box further outward, and the whole value of the gate
/// is how tightly it bounds the caller's actual reach.
const BLOCK: i32 = 2 * LATTICE_STEP;

/// Direct-mapped memo of per-block id sets, shared across worker threads
/// (values are pure functions of the key, so any thread's computation serves
/// every other).
const MEMO_BITS: u32 = 15;

#[derive(Clone, Copy)]
struct Slot {
    init: bool,
    key: Key,
    ids: IdSet,
}

/// Everything a block's id set is a function of. The PARTITION TABLE belongs in
/// here as much as the seed does: the set is a fact about a table's bands, and
/// the seed alone does not name one — a second table can be interned in the same
/// process (a test bench, a re-layered pack) and would otherwise read the first
/// one's answers out of these slots. The table is `&'static`, so its address is
/// its identity.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    seed: u32,
    table: usize,
    pos: [i32; 3],
}

static MEMO: std::sync::LazyLock<Box<[Mutex<Slot>]>> = std::sync::LazyLock::new(|| {
    (0..1usize << MEMO_BITS)
        .map(|_| {
            Mutex::new(Slot {
                init: false,
                key: Key {
                    seed: 0,
                    table: 0,
                    pos: [0; 3],
                },
                ids: IdSet::default(),
            })
        })
        .collect()
});

fn slot_idx(k: Key) -> usize {
    let key = (k.pos[0] as u32 as u64)
        ^ ((k.pos[2] as u32 as u64) << 21)
        ^ ((k.pos[1] as u32 as u64) << 42)
        ^ ((k.seed as u64) << 11)
        ^ (k.table as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
    ((key.wrapping_mul(0x9E37_79B9_7F4A_7C15)) >> (64 - MEMO_BITS)) as usize
}

impl CaveField {
    /// The conservative set of underground biome ids that can own a cell inside
    /// the inclusive world box `lo..=hi`, snapped outward to the memo grid.
    pub fn underground_biome_ids_in_box(&self, lo: [i32; 3], hi: [i32; 3]) -> IdSet {
        let mut out = IdSet::default();
        let s = |v: i32| v.div_euclid(BLOCK);
        for sy in s(lo[1])..=s(hi[1]) {
            for sz in s(lo[2])..=s(hi[2]) {
                for sx in s(lo[0])..=s(hi[0]) {
                    out.union(&self.block_ids([sx, sy, sz]));
                }
            }
        }
        out
    }

    fn block_ids(&self, sp: [i32; 3]) -> IdSet {
        let key = Key {
            seed: self.seed,
            table: std::ptr::from_ref(self.underground) as usize,
            pos: sp,
        };
        let mut slot = MEMO[slot_idx(key)]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.init && slot.key == key {
            return slot.ids;
        }
        let ids = self.compute_block_ids(sp);
        *slot = Slot {
            init: true,
            key,
            ids,
        };
        ids
    }

    fn compute_block_ids(&self, sp: [i32; 3]) -> IdSet {
        let lo = [sp[0] * BLOCK, sp[1] * BLOCK, sp[2] * BLOCK];
        let hi = [lo[0] + BLOCK - 1, lo[1] + BLOCK - 1, lo[2] + BLOCK - 1];
        let lat = self.build_lattice_filtered(
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            Fields {
                carve: false,
                interior: false,
                biome: true,
            },
        );
        let (mx, my, mz) = (lat.nx - 1, lat.ny - 1, lat.nz - 1);
        let mut out = IdSet::default();
        for cy in 0..my {
            let wy_lo = (lat.ly0 + cy as i32) * LATTICE_STEP;
            let y = (wy_lo.max(lo[1]), (wy_lo + LATTICE_STEP - 1).min(hi[1]));
            for cz in 0..mz {
                for cx in 0..mx {
                    let mut f = (f64::INFINITY, f64::NEG_INFINITY);
                    for d in 0..8usize {
                        let v = lat.biome[((cy + (d >> 2 & 1)) * lat.nz + cz + (d >> 1 & 1))
                            * lat.nx
                            + cx
                            + (d & 1)];
                        f = (f.0.min(v), f.1.max(v));
                    }
                    self.underground.ids_in(y, f, &mut out);
                }
            }
        }
        out
    }
}
