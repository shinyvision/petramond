//! Siting: the rarity roll, the coarse height scan, and the contour traces
//! read out of it — plus each trace's band-probe plan.

use mod_sdk::GenRng;

use super::{
    ADOPT_MAX, BAND_PROBE_MAX, BED_BAND, CHAIN_MAX_SAMPLES, COARSE, DAM_MAX, EDGE_TRIES, HEADROOM,
    LATTICE, LATTICE_Y, MARGIN, MAX_STEP, NSAMP, ONE_IN, PROBE_DILATE, SALT_CASCADE,
};

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
    pub fn traces(&self, rock: &[bool], free: &[bool]) -> Vec<Trace> {
        let heights = Heights::scan(self.by(), rock, free);
        let chains = heights.lip_chains();
        let mut out = Vec::new();
        for ci in rank_by_span(&chains) {
            if out.len() == EDGE_TRIES {
                break;
            }
            let chain = &chains[ci];
            if chain.len() < 2 {
                continue;
            }
            out.push(self.trace_along(&heights, chain));
        }
        out
    }

    /// The trace one contour chain offers: anchored at its highest point,
    /// nearest-the-anchor samples first, truncated to the probe budget.
    fn trace_along(&self, heights: &Heights, chain: &[(i32, i32)]) -> Trace {
        let floor = |kx: i32, kz: i32| heights.at(kx, kz).expect("chained samples have floors");
        // The anchor is the contour's highest point; the head basin floods
        // the terrace behind it.
        let &(akx, akz) = chain
            .iter()
            .max_by_key(|&&(kx, kz)| (floor(kx, kz), std::cmp::Reverse((kz, kx))))
            .expect("a chain is never empty");
        let s0 = floor(akx, akz);
        // Nearest-the-anchor samples first, so truncation keeps the part of
        // the contour the head basin actually lies along.
        let mut samples: Vec<(i32, i32, i32)> = chain
            .iter()
            .map(|&(kx, kz)| {
                let (x, z) = self.sample_col(kx, kz);
                (x, z, floor(kx, kz))
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
        t
    }
}

/// The top floor of every coarse sample column, inside the vertical pads
/// that leave room for beds below and headroom above.
struct Heights(Vec<Option<i32>>);

impl Heights {
    fn scan(by: i32, rock: &[bool], free: &[bool]) -> Heights {
        let rows = (LATTICE_Y - 2) as usize;
        let n = NSAMP as usize;
        let mut h: Vec<Option<i32>> = vec![None; n * n];
        for (i, top) in h.iter_mut().enumerate() {
            let base = i * rows;
            let (floor, air) = (&rock[base..base + rows], &free[base..base + rows]);
            // row r is world y = by + 1 + r
            let lo = (ADOPT_MAX + MAX_STEP) as usize;
            let hi = rows - 1 - HEADROOM as usize;
            // ROCK under, ROOM over: a fluid surface is neither, so a basin
            // is never sited on one.
            *top = (lo..=hi)
                .rev()
                .find(|&r| floor[r - 1] && (0..HEADROOM as usize).all(|k| air[r + k]))
                .map(|r| by + 1 + r as i32);
        }
        Heights(h)
    }

    fn at(&self, kx: i32, kz: i32) -> Option<i32> {
        (kx >= 0 && kz >= 0 && kx < NSAMP && kz < NSAMP)
            .then(|| self.0[kz as usize * NSAMP as usize + kx as usize])
            .flatten()
    }

    /// A lip sample: the floor steps down by more than the bed band within
    /// one or two samples in some direction.
    fn lip(&self, kx: i32, kz: i32) -> bool {
        let Some(me) = self.at(kx, kz) else {
            return false;
        };
        [(1, 0), (0, 1), (2, 0), (0, 2), (1, 1), (1, -1)]
            .iter()
            .any(|&(dx, dz)| {
                self.at(kx + dx, kz + dz).is_some_and(|o| me - o > BED_BAND)
                    || self.at(kx - dx, kz - dz).is_some_and(|o| me - o > BED_BAND)
            })
    }

    /// Chain lip samples along the contour: 8-connected, heights drifting no
    /// faster than the bed band between neighbours — one rolling edge each.
    fn lip_chains(&self) -> Vec<Vec<(i32, i32)>> {
        let n = NSAMP as usize;
        let mut comp: Vec<usize> = vec![usize::MAX; n * n];
        let mut chains: Vec<Vec<(i32, i32)>> = Vec::new();
        for start_kz in 0..NSAMP {
            for start_kx in 0..NSAMP {
                let si = start_kz as usize * n + start_kx as usize;
                if comp[si] != usize::MAX || !self.lip(start_kx, start_kz) {
                    continue;
                }
                let id = chains.len();
                comp[si] = id;
                let mut cells = vec![(start_kx, start_kz)];
                let mut k = 0;
                while k < cells.len() {
                    let (kx, kz) = cells[k];
                    k += 1;
                    let me = self.at(kx, kz).expect("a lip sample has a floor");
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            let (nx, nz) = (kx + dx, kz + dz);
                            if nx < 0 || nz < 0 || nx >= NSAMP || nz >= NSAMP {
                                continue;
                            }
                            let ni = nz as usize * n + nx as usize;
                            if comp[ni] == usize::MAX
                                && self.lip(nx, nz)
                                && self.at(nx, nz).is_some_and(|o| (o - me).abs() <= BED_BAND)
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
        chains
    }
}

/// Chain indices ranked by SPAN first: the long rolling edge is the design,
/// and a long thin chain beats a fat short one with more samples.
fn rank_by_span(chains: &[Vec<(i32, i32)>]) -> Vec<usize> {
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
            *chains[i].iter().min().expect("a chain is never empty"),
        )
    });
    ranked
}

/// The candidate cell's writable interior, for clamping.
#[derive(Copy, Clone)]
pub(super) struct CellBox {
    pub(super) x0: i32,
    pub(super) x1: i32,
    pub(super) z0: i32,
    pub(super) z1: i32,
    pub(super) y0: i32,
    pub(super) y1: i32,
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
    pub(super) samples: Vec<(i32, i32, i32)>,
    pub anchor: (i32, i32),
    pub s0: i32,
    pub(super) cell: CellBox,
}

impl Trace {
    /// Probe columns (sorted) with each column's row window — shared by the
    /// plan and the build so the two cannot drift.
    pub(super) fn probe_cols(&self) -> Vec<((i32, i32), (i32, i32))> {
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
}
