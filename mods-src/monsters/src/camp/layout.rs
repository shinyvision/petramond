//! Where everything in a camp stands, settled before anything is built: the plateaus and their
//! bridges, the centrepiece, the gates, towers and fortress sections on the wall line, the paths
//! and the huts. The builders read the [`Layout`] and never change it.

use std::f32::consts::TAU;

use mod_sdk::build::{Dir, Frame, Span};
use mod_sdk::{FxHashMap, FxHashSet, GenRng};

use super::grid::Grid;
use super::survey::Outline;
use super::Res;

mod centre;
mod interior;
mod relief;
mod ring;

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
}

#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
pub(super) struct Hut {
    pub kind: HutKind,
    pub frame: Frame,
    pub w: i32,
    pub d: i32,
}

fn angle_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    d.min(TAU - d)
}

/// A camp's layout, as the builders read it.
pub(super) struct Layout {
    /// Whether the camp flies a skull flag.
    pub flagged: bool,
    /// What each column is reserved for.
    pub resv: Grid<Res>,
    /// Each column's plateau id (index + 1); 0 off the plateaus.
    pub plateau_at: Grid<u8>,
    pub plateaus: Vec<Plateau>,
    pub bridges: Vec<Bridge>,
    pub centre: Option<Centre>,
    pub gates: Vec<Gate>,
    /// Per ring index, whether a gate opens there.
    pub in_gate: Vec<bool>,
    pub towers: Vec<Tower>,
    /// Per ring index, how many columns deep the fortress wall is there; 0 for the plain wall.
    pub fort: Vec<i32>,
    pub fort_arcs: Vec<FortArc>,
    /// The fortress-depth interior columns (depth 1 and 2) by the ring index they lie behind.
    pub inner_by_src: FxHashMap<usize, Vec<[i32; 2]>>,
    pub paths: Grid<bool>,
    /// The box of every column any camp pass acts on.
    pub zone: ([i32; 2], [i32; 2]),
    /// What each column of [`zone`](Layout::zone) is to the camp (`ground::IN_ZONE`, ...), row
    /// by row along x.
    pub zone_class: Vec<u8>,
    pub huts: Vec<Hut>,
    /// Open ground kept near the middle for the flag of a camp without a centrepiece.
    pub flag_spot: Option<[i32; 2]>,
}

impl Layout {
    pub(super) fn reserve(&mut self, c: [i32; 2], r: Res) {
        if self.resv.contains(c) && self.resv.get(c) == Res::Free {
            self.resv.set(c, r);
        }
    }

    fn near_plateau(&self, c: [i32; 2], r: i32) -> bool {
        (-r..=r).any(|dz| (-r..=r).any(|dx| self.plateau_at.get([c[0] + dx, c[1] + dz]) > 0))
    }
}

/// A camp being laid out: the outline it fits in and the ground the plateaus raise.
pub(super) struct Planner<'a> {
    pub(super) rng: &'a mut GenRng,
    pub(super) outline: &'a Outline,
    pub(super) ground: Grid<i32>,
    /// The box every path column lies in, once there is one.
    path_bounds: Option<([i32; 2], [i32; 2])>,
    pub(super) layout: Layout,
}

impl<'a> Planner<'a> {
    /// Lays out a camp in `outline` over `ground`. Returns the layout and the ground, raised
    /// where the plateaus stand.
    pub(super) fn lay_out(
        outline: &'a Outline,
        rng: &'a mut GenRng,
        ground: Grid<i32>,
        flagged: bool,
    ) -> (Layout, Grid<i32>) {
        let (min, size, len) = (ground.min(), ground.size(), outline.ring_len());
        let mut planner = Planner {
            rng,
            outline,
            ground,
            path_bounds: None,
            layout: Layout {
                flagged,
                resv: Grid::new(min, size, Res::Free),
                plateau_at: Grid::new(min, size, 0),
                plateaus: Vec::new(),
                bridges: Vec::new(),
                centre: None,
                gates: Vec::new(),
                in_gate: vec![false; len],
                towers: Vec::new(),
                fort: vec![0; len],
                fort_arcs: Vec::new(),
                inner_by_src: FxHashMap::default(),
                paths: Grid::new(min, size, false),
                zone: (min, min),
                zone_class: Vec::new(),
                huts: Vec::new(),
                flag_spot: None,
            },
        };
        planner.lay_plateaus();
        planner.lay_bridges();
        planner.lay_centre();
        planner.lay_flag();
        planner.lay_gates();
        planner.lay_towers();
        planner.lay_fortress();
        planner.lay_paths();
        planner.lay_huts();
        planner.settle_zone();
        (planner.layout, planner.ground)
    }

    fn g(&self, c: [i32; 2]) -> i32 {
        self.ground.get(c)
    }

    /// The columns any camp pass can act on: the ring's area and the cleared margin around it,
    /// the plateaus and the paths, clipped to the grid.
    fn zone_box(&self) -> ([i32; 2], [i32; 2]) {
        let (lo, hi) = self.outline.field.bounds();
        let m = super::ground::CLEAR_MARGIN.ceil() as i32;
        let (mut lo, mut hi) = ([lo[0] - m, lo[1] - m], [hi[0] + m, hi[1] + m]);
        let mut take = |c: [i32; 2]| {
            lo = [lo[0].min(c[0]), lo[1].min(c[1])];
            hi = [hi[0].max(c[0]), hi[1].max(c[1])];
        };
        for p in &self.layout.plateaus {
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

    /// Fixes the zone box and each of its columns' class once the layout is final.
    fn settle_zone(&mut self) {
        use super::ground::{INSIDE, IN_ZONE, ON_PATH, PLATEAU};
        let zone = self.zone_box();
        let (lo, hi) = zone;
        let mut class = Vec::with_capacity(((hi[0] - lo[0] + 1) * (hi[1] - lo[1] + 1)) as usize);
        let field = &self.outline.field;
        let (field_lo, field_hi) = field.bounds();
        for z in lo[1]..=hi[1] {
            let plateau = self.layout.plateau_at.row(z, lo[0], hi[0]);
            let paths = self.layout.paths.row(z, lo[0], hi[0]);
            let depths = (field_lo[1]..=field_hi[1])
                .contains(&z)
                .then(|| field.depth_rows().nth((z - field_lo[1]) as usize))
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
        self.layout.zone = zone;
        self.layout.zone_class = class;
    }

    fn is_plateau_top(&self, c: [i32; 2]) -> bool {
        let id = self.layout.plateau_at.get(c);
        id > 0 && self.g(c) == self.layout.plateaus[id as usize - 1].top
    }
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
