//! Bounded room and passage influence, independent of habitat surface treatment.
//!
//! Several offset lobes keep necks and pillars, and per-lobe sills make terraces. The sampler
//! blends their influence into the natural cave density and flares local tunnel entrances. Habitat
//! admission is only checked at the room anchor.
//!
//! Profiles must be exactly zero outside their declared reach. Rows and cells are summed in a fixed
//! order, so the nonzero float sum comes out identical in overlapping query windows, and a larger
//! gather can only add exact zeros.

use crate::data::excavations::{Chamber, Excavation, Excavations};
use crate::data::underground::UndergroundBiomes;
use crate::rng::FeatureRng;
use petramond_math::detmath;

use super::settings::CAVE_LATTICE_STEP;

mod cache;
pub(super) use cache::CandidateCache;
mod passage;
use passage::Passage;

const MAX_LOBES: usize = 4;

#[derive(Copy, Clone, Debug, Default)]
struct Lobe {
    off: [f64; 3],
    rx: f64,
    ry: f64,
    major: f64,
    axis: [f64; 2],
    sill_y: i32,
    sill_span: f64,
}

impl Lobe {
    /// Falloff in blocks, measured radially outward from this lobe's own surface. It's exactly
    /// `strength` inside, exactly `0.0` from `feather` blocks out, and smooth in between.
    ///
    /// On any ray the surface is at `rho_s = 1/q(u)` and the point at `rho = n/q(u)`, so
    /// `rho - rho_s = rho * (n - 1) / n` gives the radial distance exactly, without a square root.
    /// And since `rho_s * |u_y| <= ry` and `rho_s * |u_xz| <= major` for every direction, reach is
    /// bounded by `major + feather` sideways and `ry + feather` up and down. Every bound in this
    /// file depends on that.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn at(
        &self,
        y: i32,
        dx: f64,
        dy: f64,
        dz: f64,
        feather: f64,
        strength: f64,
        knead: f64,
    ) -> f64 {
        if y < self.sill_y {
            return 0.0;
        }
        let (dx, dy, dz) = (dx - self.off[0], dy - self.off[1], dz - self.off[2]);
        let along = dx * self.axis[0] + dz * self.axis[1];
        let across = dz * self.axis[0] - dx * self.axis[1];
        let horizontal = if self.major == self.rx {
            (dx * dx + dz * dz) / (self.rx * self.rx)
        } else {
            along * along / (self.major * self.major) + across * across / (self.rx * self.rx)
        };
        let n2 = horizontal + dy * dy / (self.ry * self.ry);
        let ramp = |t: f64| {
            let t = (t * knead).clamp(0.0, 1.0);
            strength * (t * t * (3.0 - 2.0 * t))
        };
        let v = if n2 <= 1.0 {
            strength
        } else {
            let n = n2.sqrt();
            let rho = (dx * dx + dy * dy + dz * dz).sqrt();
            let excess = rho * (n - 1.0) / n;
            if excess >= feather {
                return 0.0;
            }
            ramp(1.0 - excess / feather)
        };
        if self.sill_span <= 0.0 {
            return v;
        }
        v.min(ramp(((y - self.sill_y) as f64) / self.sill_span))
    }
}

#[derive(Copy, Clone, Debug)]
struct Room {
    center: [i32; 3],
    lobes: [Lobe; MAX_LOBES],
    n_lobes: usize,
    sill_y: i32,
    feather: f64,
    strength: f64,
    ex: i32,
    ey: i32,
    tunnel_gain: f64,
    rim_noise: f64,
}

impl Room {
    fn contacts(&self, open: &impl Fn([i32; 3]) -> bool) -> bool {
        let lobe = &self.lobes[0];
        for fy in [0.0, 0.25] {
            for along in [0.0, -0.5, 0.5] {
                for across in [0.0, -0.5, 0.5] {
                    let a = along * lobe.major;
                    let b = across * lobe.rx;
                    let p = [
                        self.center[0] + (a * lobe.axis[0] - b * lobe.axis[1]).round() as i32,
                        self.center[1] + (fy * lobe.ry).round() as i32,
                        self.center[2] + (a * lobe.axis[1] + b * lobe.axis[0]).round() as i32,
                    ];
                    if self.at(p[0], p[1], p[2], 0.0).0 >= self.strength && open(p) {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[inline]
    fn at(&self, x: i32, y: i32, z: i32, knead: f64) -> (f64, f64) {
        if y < self.sill_y
            || y > self.center[1] + self.ey
            || (x - self.center[0]).abs() > self.ex
            || (z - self.center[2]).abs() > self.ex
        {
            return (0.0, 0.0);
        }
        let knead = 1.0 + self.rim_noise * knead;
        let dx = (x - self.center[0]) as f64;
        let dy = (y - self.center[1]) as f64;
        let dz = (z - self.center[2]) as f64;
        let mut v: f64 = 0.0;
        for l in &self.lobes[..self.n_lobes] {
            v = v.max(l.at(y, dx, dy, dz, self.feather, self.strength, knead));
        }
        if v == 0.0 {
            return (0.0, 0.0);
        }
        (v, v * self.tunnel_gain)
    }
}

pub(super) struct ChamberField {
    rooms: Vec<Room>,
    passages: Vec<Passage>,
}

impl ChamberField {
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.rooms.is_empty() && self.passages.is_empty()
    }

    pub(super) fn restrict(&self, lo: [i32; 3], hi: [i32; 3]) -> ChamberField {
        ChamberField {
            rooms: self
                .rooms
                .iter()
                .filter(|r| r.reaches(lo, hi))
                .copied()
                .collect(),
            passages: self
                .passages
                .iter()
                .filter(|p| p.reaches(lo, hi))
                .copied()
                .collect(),
        }
    }

    #[cfg(test)]
    pub(super) fn centers(&self) -> Vec<[i32; 3]> {
        self.rooms.iter().map(|r| r.center).collect()
    }

    #[inline]
    pub(super) fn at(&self, x: i32, y: i32, z: i32, knead: f64) -> (f64, f64) {
        let (mut v, mut t) = (0.0, 0.0);
        for r in &self.rooms {
            let (rv, rt) = r.at(x, y, z, knead);
            v += rv;
            t += rt;
        }
        for passage in &self.passages {
            v += passage.at([x, y, z], knead);
        }
        (v, t)
    }

    /// Rolls every candidate room that can reach the inclusive world box `lo..=hi`.
    ///
    /// Room centres snap to the cave lattice, so sampling `biome_field_at` there gives the same
    /// value the carver's trilinear read would. Otherwise the territory test and the wall lining
    /// could disagree about who owns a room. It's also one call instead of eight.
    pub(super) fn gather(
        cache: &CandidateCache,
        table: &UndergroundBiomes,
        excavations: &Excavations,
        seed: u32,
        [lo, hi]: [[i32; 3]; 2],
        biome_field_at: impl Fn(i32, i32, i32) -> u8,
        natural_open: impl Fn([i32; 3]) -> bool,
    ) -> ChamberField {
        let mut field = ChamberField {
            rooms: Vec::new(),
            passages: Vec::new(),
        };
        let context = crate::cache::GenContext::new(seed, table, excavations);
        let candidate = |excavation: &Excavation, cx, cz| {
            let key = cache::Key::new(context, excavation, [cx, cz]);
            cache.get_or_compute(key, || {
                let eligible = |x, z| {
                    roll_room(excavation, seed, x, z).filter(|room| {
                        let Some(required) = excavation.placement.underground_biome else {
                            return true;
                        };
                        let [x, y, z] = room.center;
                        biome_field_at(x, y, z) == required
                    })
                };
                let room = eligible(cx, cz)?;
                let contact = match excavation.placement.contact {
                    None => true,
                    Some(crate::data::excavations::Contact::NaturalCave) => {
                        room.contacts(&natural_open)
                            || (excavation.connections.is_some()
                                && [(cx - 1, cz), (cx + 1, cz), (cx, cz - 1), (cx, cz + 1)]
                                    .into_iter()
                                    .any(|(x, z)| {
                                        eligible(x, z).is_some_and(|neighbor| {
                                            neighbor.contacts(&natural_open)
                                        })
                                    }))
                    }
                };
                contact.then_some(room)
            })
        };
        for excavation in &excavations.rows {
            gather_row(excavation, seed, [lo, hi], &candidate, &mut field);
        }
        field
    }
}

fn gather_row(
    excavation: &Excavation,
    seed: u32,
    [lo, hi]: [[i32; 3]; 2],
    candidate: &impl Fn(&Excavation, i32, i32) -> Option<Room>,
    out: &mut ChamberField,
) {
    let Some(chamber) = excavation.chamber() else {
        return;
    };
    let depth = Chamber::placement_band(excavation.placement.y);
    if hi[1] < depth.0 || lo[1] > depth.1 {
        return;
    }
    let spacing = excavation.placement.spacing;
    let pad = excavation.connections.map_or(chamber.reach_xz(), |c| {
        c.gather_pad(spacing).max(chamber.reach_xz())
    });
    let x0 = (lo[0] - pad).div_euclid(spacing);
    let z0 = (lo[2] - pad).div_euclid(spacing);
    let x1 = (hi[0] + pad).div_euclid(spacing);
    let z1 = (hi[2] + pad).div_euclid(spacing);
    let nx = (x1 - x0 + 1) as usize;
    let mut candidates = Vec::new();
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let room = candidate(excavation, cx, cz);
            if let Some(room) = room.filter(|r| r.reaches(lo, hi)) {
                out.rooms.push(room);
            }
            if excavation.connections.is_some() {
                candidates.push(room);
            }
        }
    }
    let Some(connections) = excavation.connections else {
        return;
    };
    for cz in z0..=z1 {
        for cx in x0..=x1 {
            let i = (cz - z0) as usize * nx + (cx - x0) as usize;
            let Some(a) = candidates[i] else {
                continue;
            };
            for (axis, neighbor) in [
                (0, (cx < x1).then_some(i + 1)),
                (1, (cz < z1).then_some(i + nx)),
            ] {
                let Some(b) = neighbor.and_then(|j| candidates[j]) else {
                    continue;
                };
                let mut rng = FeatureRng::positional(
                    seed,
                    excavation.salt ^ crate::salts::EXCAVATION_PASSAGE_XOR,
                    cx,
                    axis,
                    cz,
                );
                let passage = Passage::between(a.center, b.center, connections, &mut rng);
                if passage.reaches(lo, hi) {
                    out.passages.push(passage);
                }
            }
        }
    }
}

fn roll_room(excavation: &Excavation, seed: u32, cx: i32, cz: i32) -> Option<Room> {
    let ch = excavation.chamber()?;
    let l = excavation.placement.spacing;
    let steps = l / CAVE_LATTICE_STEP;
    let band = excavation.placement.y;
    let mut rng = FeatureRng::positional(seed, excavation.salt, cx, 0, cz);
    if rng.next_i32(0, excavation.placement.one_in - 1) != 0 {
        return None;
    }
    let jitter = |r: &mut FeatureRng| r.next_i32(0, steps - 1) * CAVE_LATTICE_STEP;
    let (jx, jz) = (jitter(&mut rng), jitter(&mut rng));
    let rx = rng.next_i32(ch.r_min, ch.r_max);
    let (mut lobes, n_lobes, ex, ey) = roll_lobes(&mut rng, ch, rx);
    let drop = ch.drop(rx);
    let passage_reach = excavation.connections.map_or(0, |c| c.reach_y());
    let cy = roll_center_y(
        &mut rng,
        band,
        drop.max(passage_reach),
        ey.max(passage_reach),
    )?;
    let room_sill = cy - drop;
    for l in lobes.iter_mut().take(n_lobes) {
        let lc = cy as f64 + l.off[1];
        l.sill_y = ((lc - l.ry * ch.sill).round() as i32).max(room_sill);
        l.sill_span = ch.feather.min(lc - l.sill_y as f64).max(0.0);
    }
    let center = [cx * l + jx, cy, cz * l + jz];
    Some(Room {
        center,
        lobes,
        n_lobes,
        sill_y: room_sill,
        feather: ch.feather,
        strength: ch.strength,
        ex,
        ey,
        tunnel_gain: ch.tunnel,
        rim_noise: ch.rim_noise,
    })
}

fn roll_lobes(rng: &mut FeatureRng, ch: &Chamber, rx: i32) -> ([Lobe; MAX_LOBES], usize, i32, i32) {
    let (rx, ry) = (rx as f64, ch.ry(rx));
    let (stretch, axis) = if ch.stretch == (1.0, 1.0) {
        (1.0, [1.0, 0.0])
    } else {
        let stretch =
            ch.stretch.0 + (ch.stretch.1 - ch.stretch.0) * rng.next_i32(0, 1000) as f64 / 1000.0;
        let yaw = rng.next_i32(0, 65535) as f64 * std::f64::consts::TAU / 65536.0;
        (stretch, [detmath::cos(yaw), detmath::sin(yaw)])
    };
    let mut lobes = [Lobe::default(); MAX_LOBES];
    lobes[0] = Lobe {
        off: [0.0; 3],
        rx,
        ry,
        major: rx * stretch,
        axis,
        ..Lobe::default()
    };
    let n = 1 + rng.next_i32(0, ch.lobes - 1) as usize;
    let frac = |r: &mut FeatureRng| r.next_i32(-1000, 1000) as f64 / 1000.0;
    for l in lobes.iter_mut().take(n).skip(1) {
        let s = ch.lobe_scale.0
            + (ch.lobe_scale.1 - ch.lobe_scale.0) * (rng.next_i32(0, 1000) as f64 / 1000.0);
        let (fx, fy, fz) = (frac(rng), frac(rng), frac(rng));
        *l = Lobe {
            off: [
                fx * ch.lobe_spread * rx,
                fy * ch.lobe_spread * ry,
                fz * ch.lobe_spread * rx,
            ],
            rx: rx * s,
            ry: ry * s,
            major: rx * s * stretch,
            axis,
            ..Lobe::default()
        };
    }
    let ex = lobes[..n]
        .iter()
        .map(|l| (l.off[0].abs().max(l.off[2].abs()) + l.major + ch.feather).ceil() as i32)
        .max()
        .unwrap_or(0);
    let ey = lobes[..n]
        .iter()
        .map(|l| (l.off[1].abs() + l.ry + ch.feather).ceil() as i32)
        .max()
        .unwrap_or(0);
    (lobes, n, ex, ey)
}

fn roll_center_y(rng: &mut FeatureRng, band: (i32, i32), drop: i32, rise: i32) -> Option<i32> {
    let step = CAVE_LATTICE_STEP;
    let band = Chamber::placement_band(band);
    let lo = band.0 + drop;
    let hi = band.1 - rise;
    let first = (lo + step - 1).div_euclid(step) * step;
    let last = hi.div_euclid(step) * step;
    if first > last {
        return None;
    }
    Some(first + rng.next_i32(0, (last - first) / step) * step)
}

impl Room {
    fn reaches(&self, lo: [i32; 3], hi: [i32; 3]) -> bool {
        let span = |c: i32, e: i32, a: i32, b: i32| c + e >= a && c - e <= b;
        span(self.center[0], self.ex, lo[0], hi[0])
            && span(self.center[2], self.ex, lo[2], hi[2])
            && self.center[1] + self.ey >= lo[1]
            && self.sill_y <= hi[1]
    }
}

#[cfg(test)]
mod connection_tests;
#[cfg(test)]
mod tests;
