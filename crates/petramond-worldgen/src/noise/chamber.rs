//! Bounded room and passage influence, independent of habitat surface treatment.
//!
//! Multiple offset lobes retain necks and pillars; per-lobe sills create terraces.
//! The sampler blends their influence into natural cave density and flares local
//! tunnel entrances. Habitat admission is checked only at the room anchor.
//!
//! Profiles must be exactly zero outside their declared reach. Canonical row and
//! cell order keeps the nonzero floating-point sum identical in overlapping query
//! windows; a larger gather may add only exact zero terms.

use crate::data::excavations::{Chamber, Excavation, Excavations};
use crate::data::underground::UndergroundBiomes;
use crate::rng::FeatureRng;

use super::settings::CAVE_LATTICE_STEP;

mod cache;
pub(super) use cache::CandidateCache;
mod passage;
use passage::Passage;

/// Hard cap on lobes per room, mirroring the loader's bound. Rooms live in a
/// `Vec` the carver walks per lattice corner, so they are stored inline.
const MAX_LOBES: usize = 4;

/// One bubble of a room. A room is the MAX of its lobes, which is what puts a
/// pinch or a pillar where two of them meet instead of blending them into one
/// convex blob.
#[derive(Copy, Clone, Debug, Default)]
struct Lobe {
    off: [f64; 3],
    rx: f64,
    ry: f64,
    major: f64,
    axis: [f64; 2],
    /// This lobe's own floor, never below the room's. A satellite riding high
    /// in the room therefore floors HIGHER than the primary, and where it
    /// reaches past the primary the floor steps up — a terrace. One sill for
    /// the whole room is a machined disc at a single Y across the entire
    /// footprint.
    sill_y: i32,
    /// Blocks above `sill_y` over which this lobe ramps in. A step here is a
    /// hard floor plane; a ramp hands the bottom of the lobe back to the noise.
    sill_span: f64,
}

impl Lobe {
    /// The falloff, measured in BLOCKS radially outward from this lobe's own
    /// surface. Exactly `strength` inside, exactly `0.0` at and past `feather`
    /// blocks outside, smooth in between.
    ///
    /// Along any ray the surface sits at `rho_s = 1/q(u)` and the point at
    /// `rho = n/q(u)`, so `rho - rho_s = rho * (n - 1) / n` — an exact radial
    /// distance, with no square root of the ellipsoid's true normal distance
    /// and no per-direction reach to measure. `rho_s * |u_y| <= ry` and
    /// `rho_s * |u_xz| <= major` hold for every direction and either aspect, so
    /// the reach is bounded by `major + feather` horizontally and `ry + feather`
    /// vertically. Every bound in this file rests on that.
    #[inline]
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
            // `knead` SCALES the ramp instead of displacing it, so it vanishes
            // where the ramp does: exactly `0.0` at the declared reach and at
            // the sill, whatever the field says. Every bound in this file rests
            // on that, and a displacement would put a step at the boundary.
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

/// One rolled room, admitted independently of its query window.
#[derive(Copy, Clone, Debug)]
struct Room {
    center: [i32; 3],
    lobes: [Lobe; MAX_LOBES],
    n_lobes: usize,
    /// The LOWEST of the lobes' sills. Below it the term is exactly zero, which
    /// is what leaves a floor — and lets an ordinary tunnel punch up through
    /// that floor instead of being capped by it.
    sill_y: i32,
    feather: f64,
    strength: f64,
    /// Precomputed half-extents, so `reaches` never re-derives them.
    ex: i32,
    ey: i32,
    /// This room's contribution to the RADIUS carvers, as a gain on the same
    /// profile. Zero for a row that declares no `chamber.tunnel`.
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

    /// `(room influence, tunnel gain)`. Both are read at every lattice
    /// corner, and both must be EXACTLY `0.0` past the declared reach.
    #[inline]
    fn at(&self, x: i32, y: i32, z: i32, knead: f64) -> (f64, f64) {
        // Past the half-extents every lobe is past its feather, so the term is
        // exactly zero there anyway; this only skips the arithmetic.
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

/// The rooms whose influence reaches one lattice box, ready to splat onto its
/// corners. Empty when no excavation reaches this box.
pub(super) struct ChamberField {
    rooms: Vec<Room>,
    passages: Vec<Passage>,
}

impl ChamberField {
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.rooms.is_empty() && self.passages.is_empty()
    }

    /// The rooms and passages of this field that can reach the inclusive box,
    /// in the same frozen order. A field gathered over a superset box holds
    /// exactly its sub-box's terms plus terms that are zero everywhere in it.
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

    /// Test seam: where the rolled rooms are, so an adversarial sweep can aim
    /// its boxes at the rims instead of hoping a random one lands on a room.
    #[cfg(test)]
    pub(super) fn centers(&self) -> Vec<[i32; 3]> {
        self.rooms.iter().map(|r| r.center).collect()
    }

    /// `(room influence, tunnel gain)` summed over every gathered room, in
    /// the frozen order (see the module docs). Both lanes accumulate from
    /// `0.0`, so a superset gather adds only exact `+0.0`.
    ///
    /// `knead` is a cave field the caller already sampled at this exact corner,
    /// so it is a pure function of position like everything else here — never
    /// a value differenced between corners, which would not be.
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

    /// Roll every candidate room that can reach the inclusive world box
    /// `lo..=hi`.
    ///
    /// `biome_field_at` samples the underground-biome field. Room centres are
    /// SNAPPED to the cave lattice, so that sample is exactly the value the
    /// carver's own trilinear read would give there — the territory test and
    /// the wall lining therefore cannot disagree about who owns the room, and
    /// the sample costs one call instead of eight.
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
        let candidate = |excavation: &Excavation, cx, cz| {
            let key = cache::Key::new(table, excavation, seed, [cx, cz]);
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
                // A neighbor must touch the independent source itself. Looking
                // up its final admission here would introduce a cyclic graph.
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
            // Positive-axis ownership emits every undirected edge once, in a
            // world order independent of the query's candidate window.
            for (axis, neighbor) in [
                (0, (cx < x1).then_some(i + 1)),
                (1, (cz < z1).then_some(i + nx)),
            ] {
                let Some(b) = neighbor.and_then(|j| candidates[j]) else {
                    continue;
                };
                let mut rng = FeatureRng::positional(
                    seed,
                    excavation.salt ^ 0x5041_5353_4147_4500,
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
    // Depth is rolled inside the row's own band, shrunk by the room's
    // ACTUAL extent, so the room stays inside the declared depth band,
    // and snapped to the cave lattice so the biome
    // sample below is exactly the value the carver reads there. The
    // loader guarantees a legal centre exists for the row's worst case,
    // so the rolled-extent window is never empty when that one is not.
    let drop = ch.drop(rx);
    let passage_reach = excavation.connections.map_or(0, |c| c.reach_y());
    let cy = roll_center_y(
        &mut rng,
        band,
        drop.max(passage_reach),
        ey.max(passage_reach),
    )?;
    // Sills, once the centre is known. Clamped to the ROOM's floor so a
    // satellite riding low simply joins it — the room's bottom stays
    // exactly `drop` below the centre, which is what every depth bound
    // is written against.
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

/// The primary lobe plus its rolled satellites, and the room's actual
/// half-extents. Satellites are offset in units of the PRIMARY's radii, so the
/// loader can bound the whole cluster from `lobe_spread + lobe_scale[1]` with
/// no per-roll reasoning, and the offsets are drawn per axis so the cluster is
/// never axis-aligned in practice.
fn roll_lobes(rng: &mut FeatureRng, ch: &Chamber, rx: i32) -> ([Lobe; MAX_LOBES], usize, i32, i32) {
    let (rx, ry) = (rx as f64, ch.ry(rx));
    let (stretch, axis) = if ch.stretch == (1.0, 1.0) {
        (1.0, [1.0, 0.0])
    } else {
        let stretch =
            ch.stretch.0 + (ch.stretch.1 - ch.stretch.0) * rng.next_i32(0, 1000) as f64 / 1000.0;
        let yaw = rng.next_i32(0, 65535) as f64 * std::f64::consts::TAU / 65536.0;
        (stretch, [yaw.cos(), yaw.sin()])
    };
    let mut lobes = [Lobe::default(); MAX_LOBES];
    // Sills are filled in once the centre is rolled; see `roll_room`.
    lobes[0] = Lobe {
        off: [0.0; 3],
        rx,
        ry,
        major: rx * stretch,
        axis,
        ..Lobe::default()
    };
    let n = 1 + rng.next_i32(0, ch.lobes - 1) as usize;
    // A fraction in [-1, 1] from one draw, so the offsets stay on the same
    // frozen stream whatever the lobe count.
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

/// A cave-lattice corner inside `band` that leaves `drop` below and `rise`
/// above it, or `None` when the band cannot hold the room (the loader rejects
/// that for the row's largest radius, but a rounding edge is cheaper to answer
/// than to argue away).
///
/// The band's low end is clipped to the carvable range: a centre below it puts
/// the sill under the world floor, where the room is sliced by bedrock into a
/// dead-flat plane of BARE rock instead of tapering to its own sill (the lining
/// shell needs the same `interior` gate the carve does, so that plane is not
/// even lined).
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
    /// Whether any cell of the inclusive box `lo..=hi` can read a non-zero term
    /// from this room. Conservative: it bounds the lobe cluster by its
    /// axis-aligned extent, so it may keep a room that happens to contribute
    /// nothing — which is harmless, because that room then adds exact zeros.
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
