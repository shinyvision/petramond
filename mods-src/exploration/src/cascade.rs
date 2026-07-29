//! Cascades: long rimstone basins traced ALONG the cave floor's own contours,
//! each spilling over its lip and down the fall line into the basin below.
//!
//! # The model
//!
//! Real rimstone terraces form along contour lines — a long sinuous lip
//! holding one water level across a slope — and the water pours over the lip
//! down the fall lines into the next terrace. This module builds exactly that:
//!
//! 1. A coarse height scan of the candidate's lattice cell finds STEP EDGES —
//!    places where the cave floor drops — and chains them into contours: runs
//!    of edge samples whose lip height drifts gradually along their length.
//!    The longest contour is the site. There is no shape to fit and no centre
//!    to roll; the terrain's own edge IS the design.
//! 2. The head basin floods the terrace behind the highest point of that
//!    contour: a region grow over columns whose floor lies within a couple of
//!    blocks under the surface, so on rolling ground the basin is a long band
//!    following the contour, wide where the ground is gentle and narrow where
//!    it steepens. Nothing is carved; the bed is the floor the carver made,
//!    lined with one course of silt, adopting natural pits as deep spots.
//! 3. Where the floor steps down past the basin's own depth band, a new basin
//!    grows on the lower terrace, seeded from the highest floor just past the
//!    lip — down the fall line, or further along a contour that keeps rolling
//!    downward. Every link gets one spill notch through the lip, and the pour
//!    is over a weir face into the pool below.
//! 4. Containment is natural rock where the terrain rises, and placed silt
//!    dams — rimstone — where it does not. A rim column that cannot be sealed
//!    within the dam budget does not reject the basin: the water RETREATS from
//!    that edge and the seal runs again. Rejection is reserved for terrain
//!    that offers no descent at all.
//!
//! # Cascades are TERRAIN; giants adapt
//!
//! A basin is part of the ground; a mushroom is something that grows on the
//! ground. The basin is therefore resolved FIRST, from the terrain alone, and
//! is never rejected because of a giant. Giants adapt to it positionally,
//! and none stands in the water: one whose body would sit in a pool, float on
//! its surface, or break the containment proof (blocking a spill,
//! intercepting a fall) is SUPPRESSED — the feature carries the suppressed
//! anchors and the giant pass skips them, in every section, because the whole
//! decision is a pure function of `(seed, cell)`.
//!
//! # The containment argument
//!
//! Water is a ticking fluid and worldgen schedules no flow check, so "it did
//! not flood when it generated" is evidence of nothing. What must hold is the
//! footprint invariant: water may flow, fall and pool freely INSIDE the
//! feature, and must never reach the ordinary cavern floor outside it.
//! Because the basin adopts cave air, that cannot be proved by construction;
//! it is proved by exhaustion: [`Trace::build`] computes the water's whole
//! REACHABLE SET over the final geometry — probed terrain, minus cut notch
//! cells, plus placed silt, plus every standing giant body — under a
//! conservative model of the fluid sim. One step past the probed domain
//! rejects; every pool must actually receive inflow or the chain is scenery.
//!
//! The model leans on four facts of `src/world/water.rs`, all load-bearing:
//! water never moves upward; a falling cell pours straight down and never
//! spreads sideways while it can; a flowing cell suspended over water never
//! spreads sideways (only sources spread across the top of water); and a fall
//! landing in source water stops dead, while one landing on solid spreads a
//! full-strength ring. Everywhere else the model over-approximates.
//!
//! # Which read may decide
//!
//! A cascade spans sections, so accept/reject must be unanimous. Every input
//! is positional: the rarity roll is `GenRng::positional` on lattice
//! coordinates, every terrain read is `terrain_solid_at`, and the giants
//! folded into the model are re-derived from their own positional rolls.
//! Nothing consults the dispatching section's snapshot, so the sections a
//! cascade straddles cannot disagree about whether it exists — which for
//! water is not a cosmetic seam but a hole in a dam.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use mod_sdk::*;

/// Frozen positional-RNG salt, append-only in practice like `cavern.rs`'s.
const SALT_CASCADE: u64 = 0x0E58_1000_0000_0006;

/// A candidate's whole footprint — pools, dams, probe shell — is confined to
/// its own lattice cell, horizontally and vertically. That confinement is a
/// containment argument, not a tidiness one: two cascades that could overlap
/// could each open a cell the other's proof holds solid, and neither would
/// ever know. Disjoint cells make the case impossible instead of rare.
///
/// The cell is 96 wide because the cell WALL is where a basin is forced to
/// stop whatever the terrain says: measured on the 0x1d001 grove, the real
/// contour edges run 39-70 blocks, and a 64-cell truncated the two longest
/// mid-landform. 96 lets most real edges fit whole; the vertical 32 is ample
/// (a whole chain descends ~8-14).
pub const LATTICE: i32 = 96;
pub const LATTICE_Y: i32 = 32;
/// Rarity is a ROLL, not a residue of rejections — this is THE frequency
/// lever, chosen deliberately: every biome cell with real relief carries a
/// cascade. The mushroom cavern is itself the rare event, and inside one the
/// water should be dependable, not a lottery; the gates below reject only
/// terrain with no descent, which in a carved cavern is the exception.
const ONE_IN: i32 = 1;
/// Footprint keeps this many columns inside the cell walls (dam + probe room).
const MARGIN: i32 = 2;
/// Coarse height-scan stride and samples per axis (covers the interior box).
const COARSE: i32 = 4;
const NSAMP: i32 = (LATTICE - 2 * MARGIN - 2) / COARSE + 1;
/// Open rows a floor needs over it to read as a floor at all.
const HEADROOM: i32 = 3;
/// How far under its surface a column's floor may lie and still belong to the
/// basin naturally. Small on purpose: it is what splits rolling ground into
/// TERRACES instead of drowning it under one deep sheet, and in-basin relief
/// within the band is the shelving of the bed.
const BED_BAND: i32 = 2;
/// Rows a bed may ADOPT downward into a natural pit enclosed by the basin —
/// a deep spot instead of a paved floor. Also the depth at which a pit gets a
/// suspended silt plate rather than adopting forever.
const ADOPT_MAX: i32 = 5;
/// Surface drop between a basin and the next one down. The minimum is
/// structural (`BED_BAND + 1`: anything gentler is the SAME basin, shelving);
/// the maximum bounds the weir face and the probe window.
const MAX_STEP: i32 = 6;
/// Basins in a chain (branches count; several pools may hang off one head).
pub const MAX_POOLS: usize = 6;
const MIN_POOLS: usize = 2;
/// Columns a basin needs to be worth keeping.
const MIN_POOL_AREA: usize = 8;
/// Contour chains attempted per rolled cell, best-scored first. The first
/// that builds owns the cell — the cell mutex that keeps footprints disjoint.
/// Four, because a cell whose longest lip fronts a chasm (a wall, not a lip)
/// often carries a humbler edge that holds a fine terrace: more tries cost
/// nothing on cells that site early and rescue cells that would get nothing.
const EDGE_TRIES: usize = 4;
/// Coarse samples a trace may keep (nearest the anchor). Bounds the probe.
/// Generous on purpose: a traced contour is usually TWO samples thick (a
/// gentle step marks the distance-2 upper sample too), so the along-edge
/// length is roughly half this in samples — the budget that caps a basin's
/// reach along its contour lives here, together with [`BAND_PROBE_MAX`].
const CHAIN_MAX_SAMPLES: usize = 44;
/// Basins may grow this far (Chebyshev) from the traced contour's samples.
const GROW_DILATE: i32 = 4;
/// The probed shell reaches this much further, so every cell the containment
/// flood can legally visit — dams, notch channels, landing rings — is probed.
/// The margin over `GROW_DILATE` must exceed the flood's 7-step spread decay.
const PROBE_DILATE: i32 = GROW_DILATE + 8;
/// Tallest silt column the seal may place (dam plus foundation). A rim that
/// needs more is a chasm; the water retreats from that edge instead.
const DAM_MAX: i32 = 6;
/// The anti-bathtub gate, keyed on dam HEIGHT, not dam share. A long low silt
/// lip meandering along a contour is what a rimstone terrace IS — on gentle
/// ground most of the rim is legitimately placed, exactly like real gours,
/// which build their own walls. The pasted-in signature is a rim of TALL
/// walls: if more than half the waterline dam columns run this many courses
/// or more, the feature is a tank standing on the floor, not a terrace.
const DAM_TALL: i32 = 4;
const DAM_TALL_SHARE_MAX: usize = 50;
/// Share of water-surface cells that must have open cave above them.
const OPEN_PERCENT: usize = 60;
/// Longest spill channel (intermediate columns) between two linked basins.
const NOTCH_PATH_MAX: usize = 6;

/// The coarse scan's size, for the budget test: every rolled cell pays this
/// once, so it must stay a couple of ABI batches.
#[cfg(test)]
pub const COARSE_PROBE: usize = (NSAMP * NSAMP) as usize * (LATTICE_Y as usize - 2);
/// Hard cap on one trace's refined band probe; [`Cell::traces`] truncates the
/// chain until its plan fits. Only edge-bearing cells in the biome ever pay
/// it, once per worker, and it is the honest price of proving a LONG basin:
/// truncating here is truncating the landform.
pub const BAND_PROBE_MAX: usize = 24 * 4096;

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// What one written cell of the feature is.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A still water SOURCE.
    Water,
    /// Placed silt: a bed course, a rimstone dam or its foundation.
    Silt,
    /// A cell cut open — a spill notch through a natural lip. The one place
    /// this feature carves, and it carves a slot for water, never a room.
    Air,
}

/// A giant mushroom whose body could cross the domain, re-derived positionally
/// by the caller. The containment flood must model exactly the giants that
/// stand — their cells win every write conflict — and the basin decides which
/// do: one in or on its water is suppressed outright, one that breaks the
/// proof is suppressed rather than the basin rejected.
pub struct Intruder {
    /// The giant's anchor lattice cell — the identity the giant pass checks.
    pub key: [i32; 3],
    /// Where its stem meets the floor.
    pub root: [i32; 3],
    pub solid: BTreeSet<[i32; 3]>,
}

/// An accepted cascade, ready to emit.
pub struct Feature {
    /// Every cell the feature writes, in one canonical order.
    pub writes: Vec<([i32; 3], Kind)>,
    /// Cells claimed but never written: the reachable wet set that is not a
    /// write, plus the cell over every water or cut cell. This is what keeps
    /// a flower off the water and a vine out of the fall.
    pub reserves: Vec<[i32; 3]>,
    /// Anchor lattice cells of giants this basin SUPPRESSES. The giant pass
    /// queries this positionally — a giant never vetoes a basin.
    pub suppressed: Vec<[i32; 3]>,
    /// The flood's full wet set, for the invariant tests.
    #[cfg(test)]
    pub wet: Vec<[i32; 3]>,
}

/// A rolled candidate cell. The roll decides only THAT this cell tries; the
/// terrain decides everything else.
pub struct Cell {
    pub lx: i32,
    pub ly: i32,
    pub lz: i32,
}

/// Every lattice cell whose box overlaps the section at `origin` (plus its
/// claim rows) — the cells whose cascade outcome this dispatch must know.
/// Pure: no host calls.
pub fn cells_overlapping(origin: [i32; 3], claim_rows: i32) -> Vec<(i32, i32, i32)> {
    cells_overlapping_box(
        origin,
        [origin[0] + 15, origin[1] + claim_rows - 1, origin[2] + 15],
    )
}

/// Every lattice cell whose box overlaps the inclusive world box — how the
/// giant pass finds the basins that could suppress a candidate whose body
/// spans `lo..=hi`.
pub fn cells_overlapping_box(lo: [i32; 3], hi: [i32; 3]) -> Vec<(i32, i32, i32)> {
    let mut out = Vec::new();
    for lz in lo[2].div_euclid(LATTICE)..=hi[2].div_euclid(LATTICE) {
        for lx in lo[0].div_euclid(LATTICE)..=hi[0].div_euclid(LATTICE) {
            for ly in lo[1].div_euclid(LATTICE_Y)..=hi[1].div_euclid(LATTICE_Y) {
                out.push((lx, ly, lz));
            }
        }
    }
    out
}

impl Cell {
    /// The rarity roll: does this lattice cell carry a candidate at all?
    /// One draw, constant count — the stream is the world's content.
    pub fn roll(seed: u32, lx: i32, ly: i32, lz: i32) -> Option<Cell> {
        let mut rng = GenRng::positional(seed, SALT_CASCADE, lx, ly, lz);
        (rng.next_i32(0, ONE_IN - 1) == 0).then_some(Cell { lx, ly, lz })
    }

    fn bx(&self) -> i32 {
        self.lx * LATTICE
    }
    fn bz(&self) -> i32 {
        self.lz * LATTICE
    }
    fn by(&self) -> i32 {
        self.ly * LATTICE_Y
    }

    /// World column of coarse sample `(kx, kz)`.
    fn sample_col(&self, kx: i32, kz: i32) -> (i32, i32) {
        (
            self.bx() + MARGIN + 1 + kx * COARSE,
            self.bz() + MARGIN + 1 + kz * COARSE,
        )
    }

    /// Cheap biome pre-gate points: the cell's centre and quadrant centres at
    /// mid-height. Any hit keeps the cell alive for the coarse scan.
    pub fn gate_points(&self) -> Vec<[i32; 3]> {
        let y = self.by() + LATTICE_Y / 2;
        let (cx, cz) = (self.bx() + LATTICE / 2, self.bz() + LATTICE / 2);
        let q = LATTICE / 4;
        vec![
            [cx, y, cz],
            [cx - q, y, cz - q],
            [cx + q, y, cz - q],
            [cx - q, y, cz + q],
            [cx + q, y, cz + q],
        ]
    }

    /// The coarse height scan: every sample column's rows, canonical order.
    pub fn coarse_plan(&self, mut f: impl FnMut([i32; 3])) {
        for kz in 0..NSAMP {
            for kx in 0..NSAMP {
                let (x, z) = self.sample_col(kx, kz);
                for y in self.by() + 1..=self.by() + LATTICE_Y - 2 {
                    f([x, y, z]);
                }
            }
        }
    }

    /// Read the coarse replies into contour TRACES: chains of step-edge
    /// samples whose lip height drifts gradually, ranked longest first.
    ///
    /// The scoring is the inversion that matters: nothing here fits a shape
    /// or rolls a centre. The terrain's longest rolling edge is the site.
    pub fn traces(&self, solid: &[bool]) -> Vec<Trace> {
        let rows = (LATTICE_Y - 2) as usize;
        let n = NSAMP as usize;
        // Top floor per sample column, inside the vertical pads that leave
        // room for beds below and headroom above.
        let mut h: Vec<Option<i32>> = vec![None; n * n];
        for kz in 0..n {
            for kx in 0..n {
                let base = (kz * n + kx) * rows;
                let col = &solid[base..base + rows];
                // row r is world y = by + 1 + r
                let lo = (ADOPT_MAX + MAX_STEP) as usize;
                let hi = rows - 1 - HEADROOM as usize;
                for r in (lo..=hi).rev() {
                    if col[r - 1] && (0..HEADROOM as usize).all(|k| !col[r + k]) {
                        h[kz * n + kx] = Some(self.by() + 1 + r as i32);
                        break;
                    }
                }
            }
        }
        let at = |kx: i32, kz: i32| -> Option<i32> {
            (kx >= 0 && kz >= 0 && kx < NSAMP && kz < NSAMP)
                .then(|| h[kz as usize * n + kx as usize])
                .flatten()
        };
        // Lip samples: the floor steps down by more than the bed band within
        // one or two samples in some direction.
        let lip = |kx: i32, kz: i32| -> bool {
            let Some(me) = at(kx, kz) else { return false };
            [(1, 0), (0, 1), (2, 0), (0, 2), (1, 1), (1, -1)]
                .iter()
                .any(|&(dx, dz)| {
                    at(kx + dx, kz + dz).is_some_and(|o| me - o > BED_BAND)
                        || at(kx - dx, kz - dz).is_some_and(|o| me - o > BED_BAND)
                })
        };
        // Chain lip samples along the contour: 8-connected, heights drifting
        // no faster than the bed band between neighbours — one rolling edge.
        let mut comp: Vec<usize> = vec![usize::MAX; n * n];
        let mut chains: Vec<Vec<(i32, i32)>> = Vec::new();
        for start_kz in 0..NSAMP {
            for start_kx in 0..NSAMP {
                let si = start_kz as usize * n + start_kx as usize;
                if comp[si] != usize::MAX || !lip(start_kx, start_kz) {
                    continue;
                }
                let id = chains.len();
                comp[si] = id;
                let mut cells = vec![(start_kx, start_kz)];
                let mut k = 0;
                while k < cells.len() {
                    let (kx, kz) = cells[k];
                    k += 1;
                    let me = at(kx, kz).unwrap();
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            let (nx, nz) = (kx + dx, kz + dz);
                            if nx < 0 || nz < 0 || nx >= NSAMP || nz >= NSAMP {
                                continue;
                            }
                            let ni = nz as usize * n + nx as usize;
                            if comp[ni] == usize::MAX
                                && lip(nx, nz)
                                && at(nx, nz).is_some_and(|o| (o - me).abs() <= BED_BAND)
                            {
                                comp[ni] = id;
                                cells.push((nx, nz));
                            }
                        }
                    }
                }
                chains.push(cells);
            }
        }
        // Rank by SPAN first: the long rolling edge is the design, and a
        // long thin chain beats a fat short one with more samples.
        let span_of = |cells: &[(i32, i32)]| {
            let (mut x0, mut x1, mut z0, mut z1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
            for &(kx, kz) in cells {
                x0 = x0.min(kx);
                x1 = x1.max(kx);
                z0 = z0.min(kz);
                z1 = z1.max(kz);
            }
            (x1 - x0).max(z1 - z0)
        };
        let mut ranked: Vec<usize> = (0..chains.len()).collect();
        ranked.sort_by_key(|&i| {
            (
                std::cmp::Reverse(span_of(&chains[i])),
                std::cmp::Reverse(chains[i].len()),
                *chains[i].iter().min().unwrap(),
            )
        });
        let mut out = Vec::new();
        for &ci in ranked.iter() {
            if out.len() == EDGE_TRIES {
                break;
            }
            let chain = &chains[ci];
            if chain.len() < 2 {
                continue;
            }
            // The anchor is the contour's highest point; the head basin
            // floods the terrace behind it.
            let &(akx, akz) = chain
                .iter()
                .max_by_key(|&&(kx, kz)| (at(kx, kz).unwrap(), std::cmp::Reverse((kz, kx))))
                .unwrap();
            let s0 = at(akx, akz).unwrap();
            // Nearest-the-anchor samples first, so truncation keeps the part
            // of the contour the head basin actually lies along.
            let mut samples: Vec<(i32, i32, i32)> = chain
                .iter()
                .map(|&(kx, kz)| {
                    let (x, z) = self.sample_col(kx, kz);
                    (x, z, at(kx, kz).unwrap())
                })
                .collect();
            let (ax, az) = self.sample_col(akx, akz);
            samples.sort_by_key(|&(x, z, _)| ((x - ax).abs().max((z - az).abs()), z, x));
            samples.truncate(CHAIN_MAX_SAMPLES);
            let mut t = Trace {
                samples,
                anchor: (ax, az),
                s0,
                cell: CellBox::of(self),
            };
            // Truncate until the band probe fits its budget.
            while t.plan_len() > BAND_PROBE_MAX && t.samples.len() > 2 {
                t.samples.pop();
            }
            out.push(t);
        }
        out
    }
}

/// The candidate cell's writable interior, for clamping.
#[derive(Copy, Clone)]
struct CellBox {
    x0: i32,
    x1: i32,
    z0: i32,
    z1: i32,
    y0: i32,
    y1: i32,
}

impl CellBox {
    fn of(c: &Cell) -> CellBox {
        CellBox {
            x0: c.bx() + MARGIN,
            x1: c.bx() + LATTICE - 1 - MARGIN,
            z0: c.bz() + MARGIN,
            z1: c.bz() + LATTICE - 1 - MARGIN,
            y0: c.by() + 1,
            y1: c.by() + LATTICE_Y - 2,
        }
    }
}

/// One contour to try: the traced lip samples, the head anchor, and the cell
/// bounds. Everything downstream is a pure function of this and the terrain.
pub struct Trace {
    /// `(x, z, lip height)` of the kept samples, anchor-nearest first.
    samples: Vec<(i32, i32, i32)>,
    pub anchor: (i32, i32),
    pub s0: i32,
    cell: CellBox,
}

/// A basin chain built against the probed terrain, containment-proven with no
/// giants. [`Built::finish`] folds the giants in and assembles the feature.
pub struct Built {
    /// Water-surface row of each pool (index = pool id). A pool emptied by
    /// the seal retreat keeps its slot with no columns.
    pub pools: Vec<i32>,
    /// Which pool spills into pool `j` (`from[0]` is unused). Read by the
    /// descent invariant tests; the shipped path bakes it into the notches.
    #[allow(dead_code)]
    pub from: Vec<usize>,
    /// Column -> (pool, bed row). Water occupies `bed+1..=surface`.
    cols: BTreeMap<(i32, i32), (usize, i32)>,
    silt: BTreeSet<[i32; 3]>,
    cuts: BTreeSet<[i32; 3]>,
    /// The no-giant reachable set (proof of the base geometry).
    reach: BTreeMap<[i32; 3], u8>,
    /// World box the probes cover, for the caller to gather giants against.
    pub domain_lo: [i32; 3],
    pub domain_hi: [i32; 3],
}

impl Trace {
    /// Probe columns (sorted) with each column's row window — shared by the
    /// plan and the build so the two cannot drift.
    fn probe_cols(&self) -> Vec<((i32, i32), (i32, i32))> {
        let (mut x0, mut x1, mut z0, mut z1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        for &(x, z, _) in &self.samples {
            x0 = x0.min(x - PROBE_DILATE);
            x1 = x1.max(x + PROBE_DILATE);
            z0 = z0.min(z - PROBE_DILATE);
            z1 = z1.max(z + PROBE_DILATE);
        }
        let mut out = Vec::new();
        for x in x0.max(self.cell.x0)..=x1.min(self.cell.x1) {
            for z in z0.max(self.cell.z0)..=z1.min(self.cell.z1) {
                let mut near = false;
                let (mut lo, mut hi) = (i32::MAX, i32::MIN);
                for &(sx, sz, sh) in &self.samples {
                    let d = (x - sx).abs().max((z - sz).abs());
                    if d <= PROBE_DILATE {
                        near = true;
                        lo = lo.min(sh);
                        hi = hi.max(sh);
                    }
                }
                if !near {
                    continue;
                }
                // Deepest read: a bed band + adopted pit + dam foundation
                // under a descended link; highest: headroom over the lip.
                let lo = (lo - (BED_BAND + ADOPT_MAX + DAM_MAX + MAX_STEP)).max(self.cell.y0);
                let hi = (hi + HEADROOM + 2).min(self.cell.y1);
                if lo <= hi {
                    out.push(((x, z), (lo, hi)));
                }
            }
        }
        out
    }

    fn plan_len(&self) -> usize {
        self.probe_cols()
            .iter()
            .map(|&(_, (lo, hi))| (hi - lo + 1) as usize)
            .sum()
    }

    /// The band probe plan, in the one canonical order the replies are read.
    pub fn plan(&self, mut f: impl FnMut([i32; 3])) {
        for ((x, z), (lo, hi)) in self.probe_cols() {
            for y in lo..=hi {
                f([x, y, z]);
            }
        }
    }

    /// Build the basin chain against the probed terrain, or say why not.
    ///
    /// `terrain` answers the probed cells (`None` past them). The build is
    /// OPTIMISTIC by design: an unsealable rim edge retreats the water, only
    /// terrain with no usable descent rejects.
    pub fn build(
        &self,
        terrain: &impl Fn([i32; 3]) -> Option<bool>,
    ) -> Result<Built, &'static str> {
        let cols_win: BTreeMap<(i32, i32), (i32, i32)> = self.probe_cols().into_iter().collect();
        let growable: BTreeSet<(i32, i32)> = cols_win
            .keys()
            .filter(|&&(x, z)| {
                self.samples
                    .iter()
                    .any(|&(sx, sz, _)| (x - sx).abs().max((z - sz).abs()) <= GROW_DILATE)
            })
            .copied()
            .collect();
        let in_win = |x: i32, z: i32, y: i32| {
            cols_win
                .get(&(x, z))
                .is_some_and(|&(lo, hi)| y >= lo && y <= hi)
        };

        // Column classification at a working surface `s`.
        #[derive(Copy, Clone, PartialEq)]
        enum Mem {
            /// Basin member; water `bed+1..=s`, bed one under the floor.
            In(i32),
            /// Floor above the surface: natural shore.
            Rock,
            /// Floor a full step below: a candidate seed for the NEXT basin
            /// (its would-be surface carried along), or too deep to use.
            Step(Option<i32>),
            /// Past the probe or the growth band.
            Off,
        }
        let classify = |x: i32, z: i32, s: i32| -> Mem {
            if !growable.contains(&(x, z)) || !in_win(x, z, s) {
                return Mem::Off;
            }
            match terrain([x, s, z]) {
                None => return Mem::Off,
                Some(true) => return Mem::Rock,
                Some(false) => {}
            }
            let mut b = s - 1;
            loop {
                match terrain([x, b, z]) {
                    None => return Mem::Off,
                    Some(true) => {
                        let depth = s - b;
                        return if depth <= BED_BAND + 1 {
                            Mem::In(b)
                        } else if depth <= MAX_STEP + 1 {
                            Mem::Step(Some(b + 1))
                        } else {
                            Mem::Step(None)
                        };
                    }
                    Some(false) => {
                        if s - b > MAX_STEP {
                            return Mem::Step(None);
                        }
                        b -= 1;
                    }
                }
            }
        };

        // --- grow the basin chain ---------------------------------------
        let grow = |s: i32,
                    seed_col: (i32, i32),
                    owned: &BTreeMap<(i32, i32), (usize, i32)>|
         -> BTreeMap<(i32, i32), i32> {
            let mut members: BTreeMap<(i32, i32), i32> = BTreeMap::new();
            let mut seen: BTreeSet<(i32, i32)> = BTreeSet::new();
            let mut frontier: BTreeSet<(i32, i32)> = BTreeSet::new();
            frontier.insert(seed_col);
            seen.insert(seed_col);
            while let Some(c) = frontier.pop_first() {
                if owned.contains_key(&c) {
                    continue;
                }
                let Mem::In(bed) = classify(c.0, c.1, s) else {
                    continue;
                };
                members.insert(c, bed);
                for (dx, dz) in SIDES {
                    let nc = (c.0 + dx, c.1 + dz);
                    if seen.insert(nc) {
                        frontier.insert(nc);
                    }
                }
            }
            members
        };

        let mut pools: Vec<i32> = Vec::new();
        let mut from: Vec<usize> = Vec::new();
        let mut cols: BTreeMap<(i32, i32), (usize, i32)> = BTreeMap::new();
        let head = grow(self.s0, self.anchor, &cols);
        if head.len() < MIN_POOL_AREA {
            return Err("head basin found no terrace");
        }
        for (c, bed) in head {
            cols.insert(c, (0, bed));
        }
        pools.push(self.s0);
        from.push(0);
        let mut banned: BTreeSet<(i32, i32)> = BTreeSet::new();
        while pools.len() < MAX_POOLS {
            // The next basin seeds from the highest floor a full step below
            // any existing basin's surface, just past its edge — the fall
            // line, or the contour continuing to roll downward.
            let mut best: Option<(i32, (i32, i32), usize)> = None;
            for (&(x, z), &(pi, _)) in &cols {
                for (dx, dz) in SIDES {
                    let nc = (x + dx, z + dz);
                    if cols.contains_key(&nc) || banned.contains(&nc) {
                        continue;
                    }
                    if let Mem::Step(Some(f)) = classify(nc.0, nc.1, pools[pi]) {
                        let cand = (f, nc, pi);
                        if best.is_none_or(|b| {
                            (cand.0, std::cmp::Reverse((cand.1, cand.2)))
                                > (b.0, std::cmp::Reverse((b.1, b.2)))
                        }) {
                            best = Some(cand);
                        }
                    }
                }
            }
            let Some((f, seed_col, pi)) = best else { break };
            let pool = grow(f, seed_col, &cols);
            if pool.len() < MIN_POOL_AREA {
                banned.insert(seed_col);
                continue;
            }
            let id = pools.len();
            for (c, bed) in pool {
                cols.insert(c, (id, bed));
            }
            pools.push(f);
            from.push(pi);
        }
        if pools.len() < MIN_POOLS {
            return Err("no descent: the floor offers no second terrace");
        }

        // --- adopt enclosed pits ------------------------------------------
        // A too-deep column ENCLOSED by one basin is a natural pit: it joins
        // as a deep spot, adopted down to `ADOPT_MAX` and floored with a
        // suspended silt plate past that. Open-sided deep ground is the
        // downhill break and stays out.
        {
            let (mut x0, mut x1, mut z0, mut z1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
            for &(x, z) in cols.keys() {
                x0 = x0.min(x);
                x1 = x1.max(x);
                z0 = z0.min(z);
                z1 = z1.max(z);
            }
            // Flood the non-members from the bbox rim; what it cannot reach
            // is enclosed.
            let mut outside: BTreeSet<(i32, i32)> = BTreeSet::new();
            let mut stack: Vec<(i32, i32)> = Vec::new();
            for x in x0 - 1..=x1 + 1 {
                for z in [z0 - 1, z1 + 1] {
                    stack.push((x, z));
                }
            }
            for z in z0 - 1..=z1 + 1 {
                for x in [x0 - 1, x1 + 1] {
                    stack.push((x, z));
                }
            }
            while let Some(c) = stack.pop() {
                if c.0 < x0 - 1 || c.0 > x1 + 1 || c.1 < z0 - 1 || c.1 > z1 + 1 {
                    continue;
                }
                if cols.contains_key(&c) || !outside.insert(c) {
                    continue;
                }
                for (dx, dz) in SIDES {
                    stack.push((c.0 + dx, c.1 + dz));
                }
            }
            let mut adopt: Vec<((i32, i32), (usize, i32))> = Vec::new();
            for x in x0..=x1 {
                for z in z0..=z1 {
                    let c = (x, z);
                    if cols.contains_key(&c) || outside.contains(&c) {
                        continue;
                    }
                    // The pit joins the pool that surrounds it: any cardinal
                    // member neighbour names it (enclosed, so one exists).
                    let Some(&(pi, _)) = SIDES
                        .iter()
                        .find_map(|&(dx, dz)| cols.get(&(x + dx, z + dz)))
                    else {
                        continue;
                    };
                    let s = pools[pi];
                    if terrain([x, s, z]) != Some(false) {
                        continue; // a rock island stays an island
                    }
                    let mut b = s - 1;
                    while s - b < ADOPT_MAX && terrain([x, b, z]) == Some(false) {
                        b -= 1;
                    }
                    if terrain([x, b, z]).is_none() {
                        continue;
                    }
                    adopt.push((c, (pi, b)));
                }
            }
            for (c, v) in adopt {
                cols.insert(c, v);
            }
        }

        // --- seal the rim, retreating where it cannot hold ----------------
        // Every water cell's four sides must be water, notch, or solid; where
        // the terrain is open, a silt dam is placed — rimstone — and carried
        // down to footing. A column needing more than `DAM_MAX` courses is a
        // chasm: the WATER RETREATS from that edge (the offending wet columns
        // leave their basin) and the seal runs again. Optimism lives here —
        // the old build rejected the whole candidate instead.
        let mut silt: BTreeSet<[i32; 3]> = BTreeSet::new();
        let mut rim_dam: BTreeSet<(i32, i32)>;
        'seal: loop {
            silt.clear();
            rim_dam = BTreeSet::new();
            let wet_col = |c: &BTreeMap<(i32, i32), (usize, i32)>, x: i32, z: i32, y: i32| {
                c.get(&(x, z))
                    .is_some_and(|&(pi, bed)| y > bed && y <= pools[pi])
            };
            // Bed courses first, so foundations can rest on them.
            for (&(x, z), &(_, bed)) in &cols {
                silt.insert([x, bed, z]);
            }
            let mut retreat: BTreeSet<(i32, i32)> = BTreeSet::new();
            for (&(x, z), &(pi, bed)) in &cols {
                let s = pools[pi];
                for y in bed + 1..=s {
                    for (dx, dz) in SIDES {
                        let (nx, nz) = (x + dx, z + dz);
                        if wet_col(&cols, nx, nz, y) {
                            continue;
                        }
                        let np = [nx, y, nz];
                        match terrain(np) {
                            Some(true) => continue,
                            None => {
                                retreat.insert((x, z));
                                continue;
                            }
                            Some(false) => {}
                        }
                        if y == s {
                            rim_dam.insert((nx, nz));
                        }
                        if !silt.insert(np) {
                            continue;
                        }
                        // Foundation: carry the dam down to something solid —
                        // rock, earlier silt, or another basin's water.
                        let mut fy = y - 1;
                        let mut height = 1;
                        loop {
                            if height > DAM_MAX {
                                retreat.insert((x, z));
                                break;
                            }
                            match terrain([nx, fy, nz]) {
                                Some(true) => break,
                                None => {
                                    retreat.insert((x, z));
                                    break;
                                }
                                Some(false) => {
                                    if wet_col(&cols, nx, nz, fy) || !silt.insert([nx, fy, nz]) {
                                        break;
                                    }
                                    height += 1;
                                    fy -= 1;
                                }
                            }
                        }
                    }
                }
            }
            if retreat.is_empty() {
                // A basin the retreat gutted is gone whole; dropping one
                // changes the geometry, so the seal runs once more over what
                // is left rather than leaving its beds and dams behind.
                let count = col_counts(pools.len(), &cols);
                let before = cols.len();
                cols.retain(|_, &mut (pi, _)| count[pi] >= MIN_POOL_AREA);
                if cols.len() == before {
                    break 'seal;
                }
                continue 'seal;
            }
            for c in retreat {
                cols.remove(&c);
            }
        }
        let count = col_counts(pools.len(), &cols);
        if count.iter().filter(|&&c| c > 0).count() < MIN_POOLS || count[0] == 0 {
            return Err("the seal retreat collapsed the chain");
        }

        // --- spill notches -------------------------------------------------
        // One channel per link: the shortest cardinal path from the lower
        // basin back to its source basin. Channel cells open at the source's
        // surface (cut through a natural lip if need be), the plunge column
        // opens down the weir face to the target's surface, and channel
        // flanks are sealed so the pour runs where it was routed.
        let mut exempt: BTreeSet<[i32; 3]> = BTreeSet::new();
        let mut cuts: BTreeSet<[i32; 3]> = BTreeSet::new();
        for j in 1..pools.len() {
            if count[j] == 0 {
                continue;
            }
            let i = from[j];
            if count[i] == 0 {
                return Err("a basin lost the terrace it spills from");
            }
            let path = notch_path(&cols, j, i).ok_or("no spill path over the lip")?;
            let s = pools[i];
            let sj = pools[j];
            // One dam-with-foundation, shared by channel and chute flanks.
            let flank = |silt: &mut BTreeSet<[i32; 3]>, np: [i32; 3]| -> Result<(), &'static str> {
                if terrain(np) == Some(false) && silt.insert(np) {
                    let mut fy = np[1] - 1;
                    let mut height = 1;
                    while height <= DAM_MAX
                        && terrain([np[0], fy, np[2]]) == Some(false)
                        && !cols
                            .get(&(np[0], np[2]))
                            .is_some_and(|&(pi, bed)| fy > bed && fy <= pools[pi])
                        && silt.insert([np[0], fy, np[2]])
                    {
                        height += 1;
                        fy -= 1;
                    }
                    if height > DAM_MAX || terrain([np[0], fy, np[2]]).is_none() {
                        return Err("a spill flank over a chasm");
                    }
                }
                Ok(())
            };
            for (k, &(x, z)) in path.iter().enumerate() {
                let last = k + 1 == path.len();
                let lo = if last { sj + 1 } else { s };
                for y in lo..=s {
                    let p = [x, y, z];
                    exempt.insert(p);
                    silt.remove(&p);
                    if terrain(p) == Some(true) {
                        cuts.insert(p);
                    }
                }
                if last {
                    // The plunge is a CHUTE: its slot stays open toward its
                    // own basin (the visible fall face) and toward whatever
                    // feeds it, and is walled everywhere else. An open
                    // lateral side is the leak class this exists for —
                    // another basin's source at slot level can convert inside
                    // the slot and sheet out onto the shore.
                    for y in lo..=s {
                        for (dx, dz) in SIDES {
                            let np = [x + dx, y, z + dz];
                            let ncol = (np[0], np[2]);
                            if exempt.contains(&np)
                                || cols.get(&ncol).is_some_and(|&(pi, _)| pi == j)
                                || cols
                                    .get(&ncol)
                                    .is_some_and(|&(pi, bed)| y > bed && y <= pools[pi])
                            {
                                continue;
                            }
                            flank(&mut silt, np)?;
                        }
                    }
                } else {
                    // Flank the channel where it is grounded; over a drop the
                    // water is already falling and falling water spreads
                    // nowhere.
                    let below = [x, s - 1, z];
                    let grounded = silt.contains(&below) || terrain(below) == Some(true);
                    if !grounded {
                        continue;
                    }
                    for (dx, dz) in SIDES {
                        let np = [x + dx, s, z + dz];
                        if exempt.contains(&np)
                            || cols
                                .get(&(np[0], np[2]))
                                .is_some_and(|&(pi, bed)| s > bed && s <= pools[pi])
                        {
                            continue;
                        }
                        flank(&mut silt, np)?;
                    }
                }
            }
        }
        // The anti-bathtub gate: measured AFTER the notches, on what actually
        // stands. Weir faces between linked basins are legitimately tall;
        // they are a small share of the rim, and half the rim running tall is
        // a tank, not a terrace.
        {
            let mut height: BTreeMap<(i32, i32), i32> = BTreeMap::new();
            for p in &silt {
                if rim_dam.contains(&(p[0], p[2])) {
                    *height.entry((p[0], p[2])).or_insert(0) += 1;
                }
            }
            let tall = height.values().filter(|&&h| h >= DAM_TALL).count();
            if tall * 100 > height.len().max(1) * DAM_TALL_SHARE_MAX {
                return Err("rim is a wall, not a lip");
            }
        }

        // --- visibility ----------------------------------------------------
        // Enough of the water must lie under open cave. Overhanging shores
        // are welcome; a flooded crack is not.
        let mut surface = 0usize;
        let mut open_sky = 0usize;
        for (&(x, z), &(pi, _)) in &cols {
            let s = pools[pi];
            surface += 1;
            if (1..=2).all(|h| terrain([x, s + h, z]) == Some(false)) {
                open_sky += 1;
            }
        }
        if surface == 0 || open_sky * 100 < surface * OPEN_PERCENT {
            return Err("water hidden under rock");
        }

        // --- the containment proof ----------------------------------------
        let wet = wet_of(&pools, &cols);
        let reach = flood(
            &pools,
            &cols,
            &wet,
            &silt,
            &cuts,
            &BTreeSet::new(),
            terrain,
            &count,
        )?;
        // The audit criterion, enforced at the source: everywhere the water
        // can ever get must lie inside the box of the water as written. The
        // flood already proved it terminates inside the PROBED shell; this is
        // the stricter "and it never sheets onto the shore" bound the
        // instrument outside judges by.
        {
            let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
            for p in &wet {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            for p in reach.keys() {
                if (0..3).any(|k| p[k] < lo[k] || p[k] > hi[k]) {
                    return Err("reach leaves the basin's own extent");
                }
            }
        }

        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for (&(x, z), &(l, h)) in cols_win.iter() {
            lo = [lo[0].min(x), lo[1].min(l), lo[2].min(z)];
            hi = [hi[0].max(x), hi[1].max(h), hi[2].max(z)];
        }
        Ok(Built {
            pools,
            from,
            cols,
            silt,
            cuts,
            reach,
            domain_lo: lo,
            domain_hi: hi,
        })
    }
}

/// Columns each pool still holds.
fn col_counts(pools_n: usize, cols: &BTreeMap<(i32, i32), (usize, i32)>) -> Vec<usize> {
    let mut count = vec![0usize; pools_n];
    for &(pi, _) in cols.values() {
        count[pi] += 1;
    }
    count
}

/// The wet set of a column map: water from over the bed up to the surface.
fn wet_of(pools: &[i32], cols: &BTreeMap<(i32, i32), (usize, i32)>) -> BTreeSet<[i32; 3]> {
    let mut wet = BTreeSet::new();
    for (&(x, z), &(pi, bed)) in cols {
        for y in bed + 1..=pools[pi] {
            wet.insert([x, y, z]);
        }
    }
    wet
}

/// Shortest cardinal path from basin `j`'s columns to basin `i`'s, through
/// unowned columns, returned source-first (the last element is the plunge
/// column inside basin `j`). Deterministic: BFS layers expand in sorted order.
fn notch_path(
    cols: &BTreeMap<(i32, i32), (usize, i32)>,
    j: usize,
    i: usize,
) -> Option<Vec<(i32, i32)>> {
    let mut parent: BTreeMap<(i32, i32), (i32, i32)> = BTreeMap::new();
    let mut frontier: VecDeque<(i32, i32)> = cols
        .iter()
        .filter(|&(_, &(pi, _))| pi == j)
        .map(|(&c, _)| c)
        .collect();
    for &c in &frontier {
        parent.insert(c, c);
    }
    for _ in 0..=NOTCH_PATH_MAX {
        let mut next = VecDeque::new();
        let mut hits: Vec<((i32, i32), (i32, i32))> = Vec::new();
        while let Some(c) = frontier.pop_front() {
            for (dx, dz) in SIDES {
                let n = (c.0 + dx, c.1 + dz);
                match cols.get(&n) {
                    Some(&(pi, _)) if pi == i => hits.push((n, c)),
                    Some(_) => {}
                    None => {
                        if !parent.contains_key(&n) {
                            parent.insert(n, c);
                            next.push_back(n);
                        }
                    }
                }
            }
        }
        if let Some(&(_, via)) = hits.iter().min() {
            let mut path = vec![via];
            let mut c = via;
            while parent[&c] != c {
                c = parent[&c];
                path.push(c);
            }
            // `path` runs from just-outside-i back into j; the last element
            // is basin j's own column — the plunge.
            return Some(path);
        }
        frontier = next;
    }
    None
}

/// The conservative reachable-set flood over the final geometry. `Err` when a
/// step leaves the probed domain or a live pool receives no inflow.
#[allow(clippy::too_many_arguments)]
fn flood(
    pools: &[i32],
    cols: &BTreeMap<(i32, i32), (usize, i32)>,
    wet: &BTreeSet<[i32; 3]>,
    silt: &BTreeSet<[i32; 3]>,
    cuts: &BTreeSet<[i32; 3]>,
    bodies: &BTreeSet<[i32; 3]>,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
    count: &[usize],
) -> Result<BTreeMap<[i32; 3], u8>, &'static str> {
    #[derive(Copy, Clone, PartialEq)]
    enum C {
        Solid,
        Open,
        Source(usize),
        Unknown,
    }
    let class = |p: [i32; 3]| -> C {
        if bodies.contains(&p) || silt.contains(&p) {
            return C::Solid;
        }
        if cuts.contains(&p) {
            return C::Open;
        }
        if wet.contains(&p) {
            return C::Source(cols[&(p[0], p[2])].0);
        }
        match terrain(p) {
            Some(true) => C::Solid,
            Some(false) => C::Open,
            None => C::Unknown,
        }
    };
    let seeds: BTreeSet<[i32; 3]> = wet.iter().copied().collect();
    let sourcey = |p: [i32; 3]| {
        seeds.contains(&p)
            || SIDES
                .iter()
                .filter(|&&(dx, dz)| seeds.contains(&[p[0] + dx, p[1], p[2] + dz]))
                .count()
                >= 2
    };
    let mut best: BTreeMap<[i32; 3], u8> = BTreeMap::new();
    let mut stack: Vec<[i32; 3]> = Vec::new();
    for &p in &seeds {
        best.insert(p, 8);
        stack.push(p);
    }
    let mut delivered = vec![false; pools.len()];
    delivered[0] = true;
    while let Some(p) = stack.pop() {
        let a = best[&p];
        let below = [p[0], p[1] - 1, p[2]];
        let mut sideways = false;
        match class(below) {
            C::Unknown => return Err("water reaches past the probed domain"),
            // Open below: the cell pours, full strength, and never spreads
            // sideways while it can.
            C::Open => {
                if best.get(&below).copied().unwrap_or(0) < 8 {
                    best.insert(below, 8);
                    stack.push(below);
                }
            }
            C::Source(k) => {
                // A fall landing in source water stops dead; only a source
                // spreads across the top of water.
                if !seeds.contains(&p) {
                    delivered[k] = true;
                }
                sideways = sourcey(p);
            }
            C::Solid => sideways = true,
        }
        if sideways && a > 1 {
            for &(dx, dz) in &SIDES {
                let q = [p[0] + dx, p[1], p[2] + dz];
                match class(q) {
                    C::Unknown => return Err("water reaches past the probed domain"),
                    C::Solid => {}
                    C::Open | C::Source(_) => {
                        if let C::Source(k) = class(q) {
                            if !seeds.contains(&p) {
                                delivered[k] = true;
                            }
                        }
                        if best.get(&q).copied().unwrap_or(0) < a - 1 {
                            best.insert(q, a - 1);
                            stack.push(q);
                        }
                    }
                }
            }
        }
    }
    for (k, (&d, &c)) in delivered.iter().zip(count.iter()).enumerate() {
        if k > 0 && c > 0 && !d {
            return Err("a basin receives no fall");
        }
    }
    Ok(best)
}

impl Built {
    /// Columns each pool holds (a pool the seal retreat emptied keeps its
    /// index with zero).
    pub fn live_counts(&self) -> Vec<usize> {
        col_counts(self.pools.len(), &self.cols)
    }

    /// Fold the giants in and assemble the feature.
    ///
    /// Giants ADAPT to the basin, never the other way round: one in or on the
    /// water is suppressed outright — no giant stands in a pool, shallow or
    /// deep — and one whose body breaks the containment proof is SUPPRESSED
    /// (greedily, in anchor order) until the proof holds again. With every
    /// interferer suppressed the geometry is the already-proven base, so the
    /// basin always survives.
    pub fn finish(
        &self,
        terrain: &impl Fn([i32; 3]) -> Option<bool>,
        intruders: &[Intruder],
    ) -> Feature {
        let mut standing: Vec<usize> = (0..intruders.len()).collect();
        standing.sort_by_key(|&k| intruders[k].key);
        let mut suppressed: Vec<[i32; 3]> = Vec::new();
        // NO giant stands in the water, shallow or deep — Rachel's rule, and
        // the end of the pedestal compromise. One whose body would sit in a
        // water cell, or whose root rests on a pool column at or under the
        // waterline (a stem standing ON the surface is still in the water),
        // is suppressed before the flood ever runs.
        standing.retain(|&k| {
            let g = &intruders[k];
            let floating = self
                .cols
                .get(&(g.root[0], g.root[2]))
                .is_some_and(|&(pi, _)| g.root[1] <= self.pools[pi] + 1);
            let wet_body = g.solid.iter().any(|p| {
                self.cols
                    .get(&(p[0], p[2]))
                    .is_some_and(|&(pi, bed)| p[1] > bed && p[1] <= self.pools[pi])
            });
            if floating || wet_body {
                suppressed.push(g.key);
                false
            } else {
                true
            }
        });
        let count = self.live_counts();
        let (reach, silt, wet) = loop {
            let mut bodies: BTreeSet<[i32; 3]> = BTreeSet::new();
            for &k in &standing {
                bodies.extend(intruders[k].solid.iter().copied());
            }
            // A body-covered cell is the giant's, not water: it leaves the
            // wet set (nothing writes it, nothing seeds from it) and the
            // body is solid to the flood — the model IS the world.
            let mut wet = wet_of(&self.pools, &self.cols);
            wet.retain(|p| !bodies.contains(p));
            match flood(
                &self.pools,
                &self.cols,
                &wet,
                &self.silt,
                &self.cuts,
                &bodies,
                terrain,
                &count,
            ) {
                Ok(reach) => break (reach, self.silt.clone(), wet),
                Err(_) if standing.is_empty() => {
                    // Unreachable in practice: with nobody standing the
                    // geometry is the base the build already proved. Kept so
                    // a caller handing in a mismatched terrain oracle loops
                    // out instead of forever.
                    break (
                        self.reach.clone(),
                        self.silt.clone(),
                        wet_of(&self.pools, &self.cols),
                    );
                }
                Err(_) => {
                    // Suppress the first standing giant that can actually be
                    // in the water's way; if none is, drop them all and the
                    // geometry is the proven base.
                    let in_the_way = |g: &Intruder| {
                        g.solid.iter().any(|p| {
                            self.reach.contains_key(p)
                                || self.cuts.contains(p)
                                || self.silt.contains(p)
                        }) || self.cols.contains_key(&(g.root[0], g.root[2]))
                    };
                    if let Some(pos) = standing.iter().position(|&k| in_the_way(&intruders[k])) {
                        suppressed.push(intruders[standing.remove(pos)].key);
                    } else {
                        suppressed.extend(standing.drain(..).map(|k| intruders[k].key));
                    }
                }
            }
        };

        // --- assemble -----------------------------------------------------
        let mut writes: BTreeMap<[i32; 3], Kind> = BTreeMap::new();
        for p in &silt {
            writes.insert(*p, Kind::Silt);
        }
        for p in &self.cuts {
            writes.insert(*p, Kind::Air);
        }
        for p in &wet {
            writes.insert(*p, Kind::Water);
        }
        let mut reserves: BTreeSet<[i32; 3]> = BTreeSet::new();
        for p in reach.keys() {
            if !writes.contains_key(p) {
                reserves.insert(*p);
            }
        }
        // The cell over every water or cut cell: both invalidate the support
        // the dressing pass judged from the pre-write snapshot.
        for (p, k) in &writes {
            if matches!(k, Kind::Water | Kind::Air) {
                let up = [p[0], p[1] + 1, p[2]];
                if !writes.contains_key(&up) {
                    reserves.insert(up);
                }
            }
        }
        Feature {
            writes: writes.into_iter().collect(),
            reserves: reserves.into_iter().collect(),
            suppressed,
            #[cfg(test)]
            wet: reach.keys().copied().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// Roll candidate cells over a spread of lattice cells, at the two
    /// vertical cells the synthetic terrain's floors actually cross.
    fn rolled(seed: u32, n: i32) -> Vec<Cell> {
        (0..n)
            .flat_map(|i| Cell::roll(seed, i * 3 + 1, -i.rem_euclid(2), i * 7 - 4))
            .collect()
    }

    /// A synthetic cave with LONG contour edges: open above a floor that
    /// descends ~3 rows every 9 columns of x (terraces running along z),
    /// folded into a triangle wave so every lattice cell holds relief, with
    /// positional roughness so the lips wander like real ground.
    fn terraced(p: [i32; 3]) -> bool {
        let t = p[0].rem_euclid(180);
        let d = t.min(180 - t);
        let rough = GenRng::positional(11, 77, p[0], 0, p[2]).next_i32(0, 2) - 1;
        let floor = 24 - 3 * (d / 9) + rough;
        p[1] < floor
    }

    /// Dead-flat floor.
    fn flat(p: [i32; 3]) -> bool {
        p[1] < 0
    }

    /// Run the whole pipeline for one cell over a synthetic terrain, exactly
    /// as the dispatcher would: coarse scan, traces, band probe, build. The
    /// probed set returned is the WINNING trace's, so a `finish` driven from
    /// it sees the same terrain the build did.
    fn run_built(c: &Cell, terrain: fn([i32; 3]) -> bool) -> Option<(Built, HashSet<[i32; 3]>)> {
        let mut coarse = Vec::new();
        c.coarse_plan(|p| coarse.push(terrain(p)));
        for t in c.traces(&coarse) {
            let mut probed = HashSet::new();
            t.plan(|p| {
                probed.insert(p);
            });
            let oracle = |p: [i32; 3]| probed.contains(&p).then(|| terrain(p));
            if let Ok(b) = t.build(&oracle) {
                return Some((b, probed));
            }
        }
        None
    }

    fn run(c: &Cell, terrain: fn([i32; 3]) -> bool) -> Option<Feature> {
        let (b, probed) = run_built(c, terrain)?;
        let oracle = |p: [i32; 3]| probed.contains(&p).then(|| terrain(p));
        Some(b.finish(&oracle, &[]))
    }

    /// Flat ground must be structurally unacceptable: no step edge exists, so
    /// no trace is even offered, and a hand-built trace finds no second
    /// terrace. This is the anti-"hole in the floor" guarantee — the failure
    /// Rachel rejected twice — enforced as a gate rather than styled around.
    #[test]
    fn a_cascade_refuses_flat_ground() {
        let mut sited = 0;
        for c in rolled(0xC0FFEE, 4000) {
            let mut coarse = Vec::new();
            c.coarse_plan(|p| coarse.push(flat(p)));
            if !c.traces(&coarse).is_empty() {
                sited += 1;
            }
        }
        assert_eq!(
            sited, 0,
            "{sited} cells offered a trace on a dead-flat floor"
        );
    }

    /// On terraced terrain the feature must actually generate — optimism is
    /// the whole point of the rework — and every accepted feature must
    /// satisfy the invariants the flood is trusted for, re-checked from the
    /// outputs alone.
    #[test]
    fn accepted_cascades_are_sealed_grounded_and_confined() {
        let mut accepted = 0;
        for c in rolled(0xC0FFEE, 400) {
            let Some(f) = run(&c, terraced) else {
                continue;
            };
            accepted += 1;
            let writes: HashMap<[i32; 3], Kind> = f.writes.iter().copied().collect();
            let wet: HashSet<[i32; 3]> = f.wet.iter().copied().collect();
            let solid_final = |p: [i32; 3]| match writes.get(&p) {
                Some(Kind::Silt) => true,
                Some(Kind::Air) | Some(Kind::Water) => false,
                None => terraced(p),
            };
            for &w in &f.wet {
                let below = [w[0], w[1] - 1, w[2]];
                assert!(
                    wet.contains(&below) || solid_final(below),
                    "wet cell {w:?} hangs over dry open ground"
                );
                if solid_final(below) {
                    for (dx, dz) in SIDES {
                        let q = [w[0] + dx, w[1], w[2] + dz];
                        assert!(
                            wet.contains(&q) || solid_final(q),
                            "grounded wet cell {w:?} is open sideways at {q:?}"
                        );
                    }
                }
            }
            // Confinement: everything inside the candidate's own lattice cell.
            let cell = |p: [i32; 3]| {
                p[0].div_euclid(LATTICE) == c.lx
                    && p[2].div_euclid(LATTICE) == c.lz
                    && p[1].div_euclid(LATTICE_Y) == c.ly
            };
            for (p, _) in &f.writes {
                assert!(cell(*p), "write {p:?} leaves the candidate's lattice cell");
            }
            for p in &f.wet {
                assert!(cell(*p), "wet {p:?} leaves the candidate's lattice cell");
            }
        }
        assert!(
            accepted >= 40,
            "only {accepted} of ~200 rolled cells accepted on terrain built to \
             carry them; the gates are wedged shut and the feature is dead"
        );
    }

    /// The basin must be LONG — it follows a contour, it is not a blob. On
    /// terrain whose terraces run the full length of the cell, an accepted
    /// feature's water must span tens of blocks along the terrace axis and
    /// read as a band, not a disc.
    #[test]
    fn a_basin_follows_the_contour_for_tens_of_blocks() {
        let mut longest = 0i32;
        let mut checked = 0;
        for c in rolled(0xC0FFEE, 400) {
            let Some(f) = run(&c, terraced) else {
                continue;
            };
            checked += 1;
            let water: Vec<[i32; 3]> = f
                .writes
                .iter()
                .filter(|(_, k)| *k == Kind::Water)
                .map(|(p, _)| *p)
                .collect();
            // terraces run along z in `terraced`
            let (mut z0, mut z1) = (i32::MAX, i32::MIN);
            for p in &water {
                z0 = z0.min(p[2]);
                z1 = z1.max(p[2]);
            }
            longest = longest.max(z1 - z0 + 1);
        }
        assert!(checked >= 40, "only {checked} features to judge");
        assert!(
            longest >= 60,
            "longest basin runs {longest} blocks along the contour; \
             a compact blob is a failure"
        );
    }

    /// The chain descends: the water sits at several heights and each basin's
    /// surface is a full step under the one that feeds it.
    #[test]
    fn accepted_cascades_descend() {
        let mut checked = 0;
        for c in rolled(0xC0FFEE, 400) {
            let Some((b, _)) = run_built(&c, terraced) else {
                continue;
            };
            checked += 1;
            let count = b.live_counts();
            let live: Vec<usize> = (0..b.pools.len()).filter(|&i| count[i] > 0).collect();
            assert!(live.len() >= MIN_POOLS);
            for &j in &live[1..] {
                assert!(
                    b.pools[j] <= b.pools[b.from[j]] - (BED_BAND + 1),
                    "basin {j} is not a full step under its source"
                );
            }
        }
        assert!(checked >= 40, "only {checked} chains to judge");
    }

    /// Probe budgets: the coarse scan is a fixed two batches, and a trace's
    /// band plan never exceeds its declared cap however the chain rolls.
    #[test]
    fn probes_stay_within_budget() {
        for c in rolled(0xC0FFEE, 400) {
            let mut n = 0usize;
            c.coarse_plan(|_| n += 1);
            assert_eq!(n, COARSE_PROBE);
            let mut coarse = Vec::new();
            c.coarse_plan(|p| coarse.push(terraced(p)));
            for t in c.traces(&coarse) {
                let mut m = 0usize;
                t.plan(|_| m += 1);
                assert!(
                    m <= BAND_PROBE_MAX,
                    "band plan probes {m} cells against the declared {BAND_PROBE_MAX}"
                );
            }
        }
    }

    /// A giant rooted in a pool is SUPPRESSED — no giant stands in water,
    /// shallow or deep, and the basin's water survives it untouched. A giant
    /// that would break containment is likewise suppressed and the basin
    /// survives — a giant never vetoes a basin.
    #[test]
    fn giants_adapt_to_the_basin_never_veto_it() {
        for c in rolled(0xC0FFEE, 400) {
            let Some((b, probed)) = run_built(&c, terraced) else {
                continue;
            };
            let oracle = |p: [i32; 3]| probed.contains(&p).then(|| terraced(p));
            let base = b.finish(&oracle, &[]);

            // A giant standing in the head basin, rooted a block over the bed.
            let (&(x, z), &(_pi, bed)) = b
                .cols
                .iter()
                .find(|&(_, &(pi, bed))| pi == 0 && b.pools[pi] - bed >= 2)
                .expect("no deep head column");
            let root = [x, bed + 2, z];
            let mut solid = BTreeSet::new();
            for dy in 0..6 {
                solid.insert([x, root[1] + dy, z]);
            }
            let stander = Intruder {
                key: [1, 2, 3],
                root,
                solid,
            };
            let f = b.finish(&oracle, &[stander]);
            assert!(
                f.suppressed.contains(&[1, 2, 3]),
                "a giant rooted in the pool at {root:?} was not suppressed"
            );
            assert!(
                !f.writes
                    .iter()
                    .any(|&(p, k)| k == Kind::Silt && p == [x, root[1] - 1, z]),
                "a pedestal was still placed under the suppressed giant"
            );
            assert_eq!(
                base.writes
                    .iter()
                    .filter(|(_, k)| *k == Kind::Water)
                    .count(),
                f.writes.iter().filter(|(_, k)| *k == Kind::Water).count(),
                "suppressing the in-pool giant changed the basin's water"
            );

            // A giant body burying every MOVING water cell (the falls and
            // spill flows — reach minus the still pools): delivery is
            // severed, containment breaks, and it is the GIANT that goes,
            // never the basin.
            let still: HashSet<[i32; 3]> = base
                .writes
                .iter()
                .filter(|(_, k)| *k == Kind::Water)
                .map(|(p, _)| *p)
                .collect();
            let wall: BTreeSet<[i32; 3]> = base
                .wet
                .iter()
                .filter(|p| !still.contains(*p))
                .copied()
                .collect();
            assert!(!wall.is_empty(), "a cascade with no moving water at all");
            let s0 = b.pools[0];
            let blocker = Intruder {
                key: [7, 8, 9],
                root: [x, s0 + 8, z],
                solid: wall,
            };
            let f = b.finish(&oracle, &[blocker]);
            assert_eq!(
                f.suppressed,
                vec![[7, 8, 9]],
                "the blocking giant was not suppressed"
            );
            let base_water: Vec<_> = base
                .writes
                .iter()
                .filter(|(_, k)| *k == Kind::Water)
                .collect();
            let f_water: Vec<_> = f.writes.iter().filter(|(_, k)| *k == Kind::Water).collect();
            assert_eq!(base_water, f_water, "suppression changed the basin's water");
            return;
        }
        panic!("no accepted candidate to test the giant path on");
    }

    /// Determinism: two builds of the same cell produce identical writes and
    /// reserves, in identical order. Everything downstream (probe reply
    /// indexing, section-unanimous emission) rests on this.
    #[test]
    fn a_build_is_deterministic() {
        let mut compared = 0;
        for c in rolled(0xC0FFEE, 200) {
            let Some(a) = run(&c, terraced) else {
                continue;
            };
            let b = run(&c, terraced).unwrap();
            assert_eq!(a.writes, b.writes);
            assert_eq!(a.reserves, b.reserves);
            assert_eq!(a.wet, b.wet);
            compared += 1;
        }
        assert!(compared > 5, "only {compared} features compared");
    }
}
