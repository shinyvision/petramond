//! Whether a site takes a camp, and the camp's outline: its ring wall's line and the columns
//! inside it, fixed from then on.

use std::f32::consts::TAU;

use mod_sdk::build::{closed_ring, depths_inside, Draw, Families, Heights, Noise2, RingField};
use mod_sdk::{smoothstep01, GenRng};

use super::grid::Grid;
use super::style::{self, Mats};
use super::{Terrain, REACH};

const MIN_R: f32 = 13.0;

/// An outline harmonic: amplitude `a` of `sin(k th + p)`, with `p`'s cosine and sine.
#[derive(Clone, Copy)]
struct Harmonic {
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

/// The camp's site and wall line.
pub(super) struct Outline {
    pub center: [i32; 2],
    pub radius: f32,
    harmonics: [Harmonic; 4],
    wobble: Noise2,
    /// The terrain's own surface over the site's reach.
    pub natural: Heights,
    pub ring: Vec<[i32; 2]>,
    pub field: RingField,
    /// The columns strictly inside the ring, in [`RingField::interior`] order.
    pub interior: Vec<[i32; 2]>,
    /// Each ring column's first index along the ring; -1 off it.
    pub ring_at: Grid<i32>,
}

impl Outline {
    pub(super) fn ring_len(&self) -> usize {
        self.ring.len()
    }

    pub(super) fn wrap(&self, i: i64) -> usize {
        i.rem_euclid(self.ring.len() as i64) as usize
    }

    pub(super) fn ring_dist(&self, a: usize, b: usize) -> usize {
        let d = a.abs_diff(b) % self.ring.len();
        d.min(self.ring.len() - d)
    }

    pub(super) fn inside(&self, c: [i32; 2]) -> bool {
        self.field.inside(c)
    }

    pub(super) fn depth(&self, c: [i32; 2]) -> i32 {
        self.field.depth(c).unwrap_or(-1)
    }

    pub(super) fn radius_at(&self, th: f32) -> f32 {
        radius_at(
            self.radius,
            &self.harmonics,
            self.wobble,
            [th.cos(), th.sin()],
        )
    }
}

/// A site that takes a camp, ready to be laid out.
pub(super) struct Surveyed<'a> {
    pub rng: GenRng,
    pub mats: Mats<'a>,
    pub outline: Outline,
    /// The natural surface, the interior levelled toward its mean.
    pub ground: Grid<i32>,
}

pub(super) enum Survey<'a> {
    Camp(Box<Surveyed<'a>>),
    Nothing,
    Unavailable,
}

pub(super) fn survey<'a>(
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
    Survey::Camp(Box::new(Surveyed {
        rng,
        mats,
        outline: Outline {
            center,
            radius,
            harmonics,
            wobble,
            natural,
            ring,
            field,
            interior,
            ring_at,
        },
        ground,
    }))
}
