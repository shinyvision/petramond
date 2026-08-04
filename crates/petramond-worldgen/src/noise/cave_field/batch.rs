//! Batched cave point queries.
//!
//! A single point query builds a degenerate one-voxel lattice: 8 world-anchored
//! corners, every field group the decision reads, for one answer. The mod ABI's
//! positional queries (`TerrainSolidAt`, `UndergroundBiomeAt`) arrive in batches
//! of hundreds, and where those cluster — column probes, section sweeps — paying
//! eight corners per position re-samples the same corners over and over.
//!
//! So a batch is solved by RECURSIVE SUBDIVISION under a cost bound: a box whose
//! lattice needs no more corners than the naive `8 × positions` is built once and
//! shared; a box that would waste corners on empty space is split at its longest
//! axis and each half re-judged. Dense batches collapse to one lattice; a batch
//! of scattered singletons degenerates to exactly the per-point cost, never worse.
//!
//! Byte-identical by construction: lattice corners are anchored on `LATTICE_STEP`
//! world coordinates, so the corner values a shared lattice carries at a position
//! are exactly the ones its own one-voxel lattice would sample, and the one
//! box-dependent filter in the build (dropping a chamber that contributes zero
//! everywhere in the box) is value-neutral by contract.

use super::{CaveCut, CaveField, Col, Fields, CAVE_MIN_Y, CAVE_SURFACE_BUFFER, LATTICE_STEP};

/// Per-position work order for the carve batch: the decision's two cheap
/// precomputed gates. Positions the gates already answered are not enqueued.
struct Carve {
    idx: u32,
    gate: Option<f64>,
    interior: bool,
}

impl CaveField {
    /// [`CaveField::cave_carved`] over a batch of `(position, column surface)`
    /// pairs, sharing one lattice per box the subdivision keeps.
    pub fn cave_carved_batch(&self, queries: &[([i32; 3], i32)], out: &mut Vec<bool>) {
        out.clear();
        out.resize(queries.len(), false);

        // Gates first, exactly as the point path orders them: they are exact,
        // cheap, and they reject the overwhelming majority of positions without
        // any lattice at all.
        let mut work: Vec<Carve> = Vec::new();
        for (i, ([x, y, z], surf_y)) in queries.iter().enumerate() {
            let (x, y, z, surf_y) = (*x, *y, *z, *surf_y);
            if y > surf_y {
                continue;
            }
            let gate = self.entrance_gate_ease(x, y, z, surf_y);
            let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
            if gate.is_none() && !interior {
                continue;
            }
            work.push(Carve {
                idx: i as u32,
                gate,
                interior,
            });
        }

        subdivide(
            &mut work,
            |w| queries[w.idx as usize].0,
            &mut |group, lo, hi| {
                let any_interior = group.iter().any(|w| w.interior);
                let fields = Fields {
                    carve: true,
                    interior: any_interior,
                    biome: any_interior && self.caliber_varies,
                };
                let lat =
                    self.build_lattice_filtered(lo[0], lo[1], lo[2], hi[0], hi[1], hi[2], fields);
                // Column-major within the group: probes arrive as column scans,
                // and one cursor per column hoists the x/z interpolation out of
                // the run exactly as the batch carve's walk does.
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
                    out[w.idx as usize] =
                        self.cut_from_col(c, y, w.gate, w.interior) == CaveCut::Open;
                }
            },
        );
    }

    /// [`CaveField::underground_biome_at`] over a batch, same subdivision.
    pub fn underground_biome_at_batch(&self, positions: &[[i32; 3]], out: &mut Vec<u8>) {
        out.clear();
        out.resize(positions.len(), 0);
        const FIELDS: Fields = Fields {
            carve: false,
            interior: false,
            biome: true,
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
                    out[i as usize] = self.underground.id_at(c.get(super::lane::BIOME, y), y);
                }
            },
        );
    }
}

/// Lattice corners an inclusive world box needs.
fn corner_count(lo: [i32; 3], hi: [i32; 3]) -> u64 {
    (0..3)
        .map(|a| (hi[a].div_euclid(LATTICE_STEP) - lo[a].div_euclid(LATTICE_STEP) + 2) as u64)
        .product()
}

/// Split `items` until every group's shared lattice costs no more corners than
/// solving its positions one at a time, then hand each group to `eval`.
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
    // Longest axis, halved on a lattice boundary so neither side inherits the
    // other's corner column.
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

/// In-place stable-enough partition (order within a side is irrelevant — every
/// answer is written back through the item's own index). Returns the pivot.
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
