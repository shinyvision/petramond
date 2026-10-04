use std::f32::consts::TAU;

use mod_sdk::build::{closed_ring, depths_inside, Dir, Draw, Families, Frame, Noise2, Plan, Span};
use mod_sdk::{smoothstep01, FxHashMap, FxHashSet, GenRng};

use super::grid::Grid;
use super::style::{self, Mats};
use super::{Camp, Res, Survey, Terrain, REACH};

mod centre;
mod interior;
mod relief;
mod ring;

const MIN_R: f32 = 13.0;

pub(super) struct Plateau {
    pub id: u8,
    pub center: [f32; 2],
    pub top: i32,
    pub cells: Vec<[i32; 2]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Rails {
    Both,
    One,
    Neither,
}

pub(super) struct Bridge {
    pub span: Span,
    pub rails: Rails,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CentreKind {
    Well,
    Statue,
    Arena,
}

pub(super) struct Walkway {
    pub dir: [f32; 2],
    pub rim: f32,
    pub tip: f32,
    pub far: f32,
    pub cells: Vec<([i32; 2], f32)>,
}

pub(super) struct Centre {
    pub kind: CentreKind,
    pub at: [i32; 2],
    pub need: f32,
    pub ra: f32,
    /// The pit outline's harmonics k = 2, 3: amplitude and the phase's cosine and sine.
    pub pit_shape: [(f32, f32, f32); 2],
    pub pit: FxHashSet<[i32; 2]>,
    pub walkways: Vec<Walkway>,
}

impl Centre {
    pub fn pit_radius(&self, a: f32) -> f32 {
        self.pit_radius_toward(a.cos(), a.sin())
    }

    /// The pit's radius in the direction `(cos a, sin a)`.
    pub fn pit_radius_toward(&self, cos: f32, sin: f32) -> f32 {
        let (c2, s2) = (cos * cos - sin * sin, 2.0 * sin * cos);
        let (c3, s3) = (c2 * cos - s2 * sin, s2 * cos + c2 * sin);
        let [(a2, cp2, sp2), (a3, cp3, sp3)] = self.pit_shape;
        self.ra + self.ra * a2 * (s2 * cp2 + c2 * sp2) + self.ra * a3 * (s3 * cp3 + c3 * sp3)
    }
}

pub(super) struct Gate {
    pub i: usize,
    pub idx: Vec<usize>,
    pub from: usize,
    pub to: usize,
    pub out: [f32; 2],
}

pub(super) struct Tower {
    pub s: i32,
    pub min: [i32; 2],
    pub dir: Dir,
    /// The y of the platform's floor blocks, once the tower is built.
    pub top: i32,
}

pub(super) struct FortArc {
    pub center: usize,
    pub span: i32,
    pub full: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum HutKind {
    Hut,
    LeanTo,
    AFrame,
}

pub(super) struct Hut {
    pub kind: HutKind,
    pub frame: Frame,
    pub w: i32,
    pub d: i32,
}

/// An outline harmonic: amplitude `a` of `sin(k th + p)`, with `p`'s cosine and sine.
#[derive(Clone, Copy)]
pub(super) struct Harmonic {
    k: u32,
    a: f32,
    cos_p: f32,
    sin_p: f32,
}

/// The outline's radius in the direction `(cos th, sin th)`.
fn radius_at(radius: f32, harmonics: &[Harmonic; 4], wobble: Noise2, [cos, sin]: [f32; 2]) -> f32 {
    // (c, s) is (cos k th, sin k th), stepped up k by angle addition.
    let (mut c, mut s, mut k) = (cos, sin, 1);
    let mut f = 1.0;
    for h in harmonics {
        while k < h.k {
            (c, s) = (c * cos - s * sin, s * cos + c * sin);
            k += 1;
        }
        f += h.a * (s * h.cos_p + c * h.sin_p);
    }
    f += 0.03 * wobble.at(cos * 3.0 + 10.0, sin * 3.0 + 10.0);
    (radius * f).max(MIN_R)
}

fn angle_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

impl<'a> Camp<'a> {
    pub(super) fn survey(
        center: [i32; 2],
        sea_level: i32,
        families: &'a Families,
        terrain: &dyn Terrain,
        mut rng: GenRng,
    ) -> Survey<'a> {
        let Some(biome) = terrain.biomes(vec![center]) else {
            return Survey::Unavailable;
        };
        let Some(style) = style::for_biome(biome[0]) else {
            return Survey::Nothing;
        };
        let min = [center[0] - REACH, center[1] - REACH];
        let max = [center[0] + REACH, center[1] + REACH];
        let Some(natural) = terrain.heights(min, max) else {
            return Survey::Unavailable;
        };
        let mats = Mats::new(families, style, &mut rng);
        let sizes = [
            (rng.range(13.0, 17.0), 4.0),
            (rng.range(17.0, 22.0), 4.0),
            (rng.range(22.0, 27.0), 2.0),
        ];
        let radius = *rng.weighted(&sizes);
        let harmonics = [(2, 0.09), (3, 0.065), (4, 0.035), (5, 0.025)].map(|(k, a)| {
            let (a, p) = (rng.range(0.0, a), rng.range(0.0, TAU));
            Harmonic {
                k,
                a,
                cos_p: p.cos(),
                sin_p: p.sin(),
            }
        });
        let wobble = Noise2(rng.next_u64() as u32);
        let ring = closed_ring(center, radius, |cos, sin| {
            radius_at(radius, &harmonics, wobble, [cos, sin])
        });
        let field = depths_inside(&ring, center);

        let interior: Vec<[i32; 2]> = field.interior().collect();
        // The footprint (ring and interior) is every column at depth 0 or more, inside the
        // ring's bounds: the field's one-column frame is outside.
        let (lo, hi) = field.bounds();
        let (x0, x1) = (lo[0] + 1, hi[0] - 1);
        let footprint_rows = || {
            field
                .depth_rows()
                .zip(lo[1]..)
                .skip(1)
                .take((hi[1] - lo[1] - 1) as usize)
                .map(move |(depths, z)| (z, &depths[1..depths.len() - 1]))
        };
        let (mut sum, mut count) = (0.0f32, 0usize);
        for (z, depths) in footprint_rows() {
            let Some(heights) = natural.row(z, x0, x1) else {
                return Survey::Nothing;
            };
            for (&d, &n) in depths.iter().zip(heights) {
                if d >= 0 {
                    if n < sea_level {
                        return Survey::Nothing;
                    }
                    sum += n as f32;
                    count += 1;
                }
            }
        }
        let mean = sum / count as f32;
        let (mut spread, mut squares) = (0.0f32, 0.0f32);
        for (z, depths) in footprint_rows() {
            let heights = natural.row(z, x0, x1).unwrap_or_default();
            for (&d, &n) in depths.iter().zip(heights) {
                if d >= 0 {
                    let off = n as f32 - mean;
                    spread = spread.max(off.abs());
                    squares += off * off;
                }
            }
        }
        if spread > 10.0 || (squares / count as f32).sqrt() > 3.5 {
            return Survey::Nothing;
        }
        let mut wet: Vec<[i32; 3]> = Vec::new();
        for (z, depths) in footprint_rows().filter(|(z, _)| z.rem_euclid(5) == 0) {
            let heights = natural.row(z, x0, x1).unwrap_or_default();
            let first = (5 - x0.rem_euclid(5)) % 5;
            for i in (first as usize..depths.len()).step_by(5) {
                if depths[i] >= 0 {
                    wet.push([x0 + i as i32, heights[i] + 1, z]);
                }
            }
        }
        match terrain.fluid(wet) {
            None => return Survey::Unavailable,
            Some(fluid) if fluid.iter().any(|&f| f) => return Survey::Nothing,
            Some(_) => {}
        }

        let size = 2 * REACH + 1;
        let mut ground = Grid::from_cells(min, size, natural.values().to_vec(), sea_level);
        // Interior columns lean toward the mean with depth; the ring itself keeps its height.
        let lean = [1, 2, 3, 4].map(|d| 0.55 * smoothstep01((d as f32 / 4.0).min(1.0)));
        let (lo, hi) = field.bounds();
        for (depths, z) in field.depth_rows().zip(lo[1]..) {
            let row = ground.row_mut(z, lo[0], hi[0]);
            for (g, &d) in row.iter_mut().zip(depths) {
                if d > 0 {
                    let w = lean[d.min(4) as usize - 1];
                    *g = (*g as f32 * (1.0 - w) + mean * w).round_ties_even() as i32;
                }
            }
        }
        let mut ring_at = Grid::new(min, size, -1);
        for (i, &c) in ring.iter().enumerate() {
            if ring_at.get(c) < 0 {
                ring_at.set(c, i as i32);
            }
        }
        let len = ring.len();
        Survey::Camp(Box::new(Camp {
            rng,
            mats,
            plan: Plan::new(),
            center,
            radius,
            harmonics,
            wobble,
            natural,
            ground,
            ring,
            field,
            interior,
            ring_at,
            plateau_at: Grid::new(min, size, 0),
            resv: Grid::new(min, size, Res::Free),
            plateaus: Vec::new(),
            bridges: Vec::new(),
            centre: None,
            gates: Vec::new(),
            in_gate: vec![false; len],
            towers: Vec::new(),
            fort: vec![0; len],
            fort_arcs: Vec::new(),
            fort_cut: vec![0; len],
            inner_by_src: FxHashMap::default(),
            paths: Grid::new(min, size, false),
            path_bounds: None,
            zone: (min, max),
            zone_class: Vec::new(),
            huts: Vec::new(),
            wall_h: vec![0; len],
            rubble: Vec::new(),
            posts: Vec::new(),
            flagged: false,
            standard: None,
        }))
    }

    pub(super) fn radius_at(&self, th: f32) -> f32 {
        radius_at(
            self.radius,
            &self.harmonics,
            self.wobble,
            [th.cos(), th.sin()],
        )
    }

    /// The columns any camp pass can act on: the ring's area and the cleared margin around it,
    /// the plateaus and the paths, clipped to the grid.
    pub(super) fn zone_box(&self) -> ([i32; 2], [i32; 2]) {
        let (lo, hi) = self.field.bounds();
        let m = super::ground::CLEAR_MARGIN.ceil() as i32;
        let (mut lo, mut hi) = ([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m]);
        let mut take = |c: [i32; 2]| {
            lo = [lo[0].min(c[0]), lo[1].min(c[1])];
            hi = [hi[0].max(c[0]), hi[1].max(c[1])];
        };
        for p in &self.plateaus {
            p.cells.iter().for_each(|&c| take(c));
        }
        if let Some((a, b)) = self.path_bounds {
            take(a);
            take(b);
        }
        let (g0, n) = (self.ground.min(), self.ground.size());
        (
            [lo[0].max(g0[0]), lo[1].max(g0[1])],
            [hi[0].min(g0[0] + n - 1), hi[1].min(g0[1] + n - 1)],
        )
    }

    pub(super) fn lay_out(&mut self) {
        self.lay_plateaus();
        self.lay_bridges();
        self.lay_centre();
        self.lay_flag();
        self.lay_gates();
        self.lay_towers();
        self.lay_fortress();
        self.lay_paths();
        self.lay_huts();
        self.settle_zone();
    }

    /// Fixes the zone box and each of its columns' class once the layout is final.
    pub(super) fn settle_zone(&mut self) {
        use super::ground::{INSIDE, IN_ZONE, ON_PATH, PLATEAU};
        self.zone = self.zone_box();
        let (lo, hi) = self.zone;
        let mut class = Vec::with_capacity(((hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1)) as usize);
        let (field_lo, field_hi) = self.field.bounds();
        for z in lo[1]..=hi[1] {
            let plateau = self.plateau_at.row(z, lo[0], hi[0]);
            let paths = self.paths.row(z, lo[0], hi[0]);
            let depths = (field_lo[1]..=field_hi[1])
                .contains(&z)
                .then(|| self.field.depth_rows().nth((z - field_lo[1]) as usize))
                .flatten();
            for ((&p, &on_path), x) in plateau.iter().zip(paths).zip(lo[0]..) {
                let depth = match depths {
                    Some(row) if (field_lo[0]..=field_hi[0]).contains(&x) => {
                        row[(x - field_lo[0]) as usize] as i32
                    }
                    _ => -1,
                };
                let plateau = p > 0;
                class.push(
                    (u8::from(depth >= 0 || plateau) * IN_ZONE)
                        | (u8::from(depth > 0) * INSIDE)
                        | (u8::from(plateau) * PLATEAU)
                        | (u8::from(on_path) * ON_PATH),
                );
            }
        }
        self.zone_class = class;
    }

    fn is_plateau_top(&self, c: [i32; 2]) -> bool {
        let id = self.plateau_at.get(c);
        id > 0 && self.g(c) == self.plateaus[id as usize - 1].top
    }

    fn near_plateau(&self, c: [i32; 2], r: i32) -> bool {
        (-r..=r).any(|dz| (-r..=r).any(|dx| self.plateau_at.get([c[0] + dx, c[1] + dz]) > 0))
    }

    // Raised rock shelves straddling the wall line; the wall runs over them.
}

/// [`disc`] as rows: `(dz, half)` for each row of the disc around the origin, which spans
/// `-half..=half`.
pub(super) fn disc_rows(r: f32) -> Vec<(i32, i32)> {
    let mut rows: Vec<(i32, i32)> = Vec::new();
    for [dx, dz] in disc([0, 0], r) {
        match rows.last_mut() {
            Some((z, half)) if *z == dz => *half = (*half).max(dx),
            _ => rows.push((dz, dx.abs())),
        }
    }
    rows
}

/// Running counts of the columns meeting a test over a box, so any run of a row is checked in
/// constant time.
pub(super) struct RowCounts {
    min: [i32; 2],
    max: [i32; 2],
    counts: Vec<u32>,
}

impl RowCounts {
    /// Counts over `min..=max` from each row's test results along x, row by row along z.
    pub(super) fn from_rows<R: IntoIterator<Item = bool>>(
        min: [i32; 2],
        max: [i32; 2],
        rows: impl IntoIterator<Item = R>,
    ) -> RowCounts {
        let width = (max[0] - min[0] + 2) as usize;
        let mut counts = Vec::with_capacity(width * (max[1] - min[1] + 1) as usize);
        for row in rows {
            let mut n = 0;
            counts.push(0);
            for ok in row {
                n += u32::from(ok);
                counts.push(n);
            }
        }
        RowCounts { min, max, counts }
    }

    /// Whether every column of row `z` from `x0` to `x1` meets the test (columns off the box
    /// never do).
    pub(super) fn all(&self, z: i32, x0: i32, x1: i32) -> bool {
        if z < self.min[1] || z > self.max[1] || x0 < self.min[0] || x1 > self.max[0] {
            return false;
        }
        let row = (z - self.min[1]) as usize * (self.max[0] - self.min[0] + 2) as usize;
        let (a, b) = ((x0 - self.min[0]) as usize, (x1 - self.min[0]) as usize + 1);
        self.counts[row + b] - self.counts[row + a] == (x1 - x0 + 1) as u32
    }

    /// Whether every column of the disc `rows` around `c` meets the test.
    pub(super) fn all_in_disc(&self, c: [i32; 2], rows: &[(i32, i32)]) -> bool {
        rows.iter()
            .all(|&(dz, half)| self.all(c[1] + dz, c[0] - half, c[0] + half))
    }
}

/// Columns within `r` of `c`.
pub(super) fn disc(c: [i32; 2], r: f32) -> impl Iterator<Item = [i32; 2]> {
    let n = r.ceil() as i32;
    (-n..=n).flat_map(move |dz| {
        (-n..=n).filter_map(move |dx| {
            ((dx * dx + dz * dz) as f32 <= r * r).then_some([c[0] + dx, c[1] + dz])
        })
    })
}
