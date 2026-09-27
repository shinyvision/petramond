use mod_sdk::GenRng;

use super::{
    ADOPT_MAX, BAND_PROBE_MAX, BED_BAND, CHAIN_MAX_SAMPLES, COARSE, DAM_MAX, EDGE_TRIES, HEADROOM,
    LATTICE, LATTICE_Y, MARGIN, MAX_STEP, NSAMP, ONE_IN, PROBE_DILATE, SALT_CASCADE,
};

pub struct Cell {
    pub lx: i32,
    pub ly: i32,
    pub lz: i32,
}

pub fn cells_overlapping(origin: [i32; 3], claim_rows: i32) -> Vec<(i32, i32, i32)> {
    cells_overlapping_box(
        origin,
        [origin[0] + 15, origin[1] + claim_rows - 1, origin[2] + 15],
    )
}

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

    fn sample_col(&self, kx: i32, kz: i32) -> (i32, i32) {
        (
            self.bx() + MARGIN + 1 + kx * COARSE,
            self.bz() + MARGIN + 1 + kz * COARSE,
        )
    }

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

    fn trace_along(&self, heights: &Heights, chain: &[(i32, i32)]) -> Trace {
        let floor = |kx: i32, kz: i32| heights.at(kx, kz).expect("chained samples have floors");
        let &(akx, akz) = chain
            .iter()
            .max_by_key(|&&(kx, kz)| (floor(kx, kz), std::cmp::Reverse((kz, kx))))
            .expect("a chain is never empty");
        let s0 = floor(akx, akz);
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
        while t.plan_len() > BAND_PROBE_MAX && t.samples.len() > 2 {
            t.samples.pop();
        }
        t
    }
}

struct Heights(Vec<Option<i32>>);

impl Heights {
    fn scan(by: i32, rock: &[bool], free: &[bool]) -> Heights {
        let rows = (LATTICE_Y - 2) as usize;
        let n = NSAMP as usize;
        let mut h: Vec<Option<i32>> = vec![None; n * n];
        for (i, top) in h.iter_mut().enumerate() {
            let base = i * rows;
            let (floor, air) = (&rock[base..base + rows], &free[base..base + rows]);
            let lo = (ADOPT_MAX + MAX_STEP) as usize;
            let hi = rows - 1 - HEADROOM as usize;
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

pub struct Trace {
    pub(super) samples: Vec<(i32, i32, i32)>,
    pub anchor: (i32, i32),
    pub s0: i32,
    pub(super) cell: CellBox,
}

impl Trace {
    pub(super) fn probe_cols(&self) -> Vec<((i32, i32), (i32, i32))> {
        let width = (self.cell.z1 - self.cell.z0 + 1) as usize;
        let height = (self.cell.x1 - self.cell.x0 + 1) as usize;
        let mut windows = vec![(i32::MAX, i32::MIN); width * height];
        let slot =
            |x: i32, z: i32| (x - self.cell.x0) as usize * width + (z - self.cell.z0) as usize;
        for &(sx, sz, sh) in &self.samples {
            for x in (sx - PROBE_DILATE).max(self.cell.x0)..=(sx + PROBE_DILATE).min(self.cell.x1) {
                for z in
                    (sz - PROBE_DILATE).max(self.cell.z0)..=(sz + PROBE_DILATE).min(self.cell.z1)
                {
                    let (lo, hi) = &mut windows[slot(x, z)];
                    *lo = (*lo).min(sh);
                    *hi = (*hi).max(sh);
                }
            }
        }
        let mut out = Vec::new();
        for x in self.cell.x0..=self.cell.x1 {
            for z in self.cell.z0..=self.cell.z1 {
                let (lo, hi) = windows[slot(x, z)];
                if lo > hi {
                    continue;
                }
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

    pub fn plan(&self, mut f: impl FnMut([i32; 3])) {
        for ((x, z), (lo, hi)) in self.probe_cols() {
            for y in lo..=hi {
                f([x, y, z]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_window_matches_each_samples_square_and_height() {
        let cell = Cell {
            lx: 0,
            ly: 0,
            lz: -1,
        };
        let trace = Trace {
            samples: vec![(3, -94, 25), (23, -80, 14), (40, -85, 20)],
            anchor: (3, -94),
            s0: 25,
            cell: CellBox::of(&cell),
        };
        let mut expected = Vec::new();
        for x in trace.cell.x0..=trace.cell.x1 {
            for z in trace.cell.z0..=trace.cell.z1 {
                let heights: Vec<_> = trace
                    .samples
                    .iter()
                    .filter_map(|&(sx, sz, sh)| {
                        ((x - sx).abs().max((z - sz).abs()) <= PROBE_DILATE).then_some(sh)
                    })
                    .collect();
                if let (Some(lo), Some(hi)) = (heights.iter().min(), heights.iter().max()) {
                    let lo = (*lo - (BED_BAND + ADOPT_MAX + DAM_MAX + MAX_STEP)).max(trace.cell.y0);
                    let hi = (*hi + HEADROOM + 2).min(trace.cell.y1);
                    if lo <= hi {
                        expected.push(((x, z), (lo, hi)));
                    }
                }
            }
        }
        assert_eq!(trace.probe_cols(), expected);
    }
}
