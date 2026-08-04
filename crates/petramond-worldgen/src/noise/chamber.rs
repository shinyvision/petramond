//! Data-declared rooms summed into the cavern carve threshold.
//!
//! A [`Chamber`] row does not STAMP anything. It contributes an additive term
//! to the value the cheese carver thresholds, sampled on the same world-anchored
//! lattice as every other cave field. Where the term is large the threshold
//! clears the sampler's whole range and the cell is open unconditionally — a
//! real room. Where it is small the threshold is merely nudged and the noise
//! still decides, so the rim comes out as knuckles, alcoves and struts, and a
//! natural tunnel running into it simply merges. That blending is the entire
//! point: it is what "a modded carver EXTENDS the engine's carvers" means, as
//! opposed to a boolean union against finished geometry.
//!
//! A room emits a SECOND term on the same profile, the tunnel gain, because
//! merging on the threshold alone is only possible where the cavern field is
//! already near its threshold. Where it is not, the rim changes nothing and the
//! room comes out sealed however wide the rim is. The gain rides the RADIUS
//! carvers instead, which are live everywhere, so a tunnel passing the rim
//! flares open into the room — and flares LINED, since the shell test uses the
//! fattened radius.
//!
//! Three things stop a room reading as an ellipsoid, and all three are shape,
//! not noise: it is the MAX of several offset lobes (a sum would blend them
//! back into one convex blob; a max leaves a pinch or a pillar in the crease),
//! its rim is measured in BLOCKS rather than in normalised radius (so
//! flattening the room no longer flattens its ceiling's rim into a smooth
//! analytic dome), and every lobe carries its OWN ramped sill (one hard sill
//! for the whole room is a machined disc of floor at a single Y across the
//! entire footprint; per-lobe sills terrace it).
//!
//! # Two invariants this file exists to keep
//!
//! PURITY ACROSS BOXES. Lattice corners are shared between neighbouring boxes,
//! so a corner's value must not depend on which box gathered it. Two things
//! make that true: a room's profile is EXACTLY `0.0` past its declared reach
//! (never a denormal tail), and rooms accumulate from `0.0` in a frozen order.
//! A superset gather therefore adds only exact `+0.0` terms, which cannot
//! perturb an f64 sum, and the non-zero subsequence is identical. f64 addition
//! is not associative, so this is a real hazard, not a theoretical one — a
//! last-ulp difference would show up as a carve seam on some section boundaries
//! on some seeds and nowhere else.
//!
//! BOUNDEDNESS. The loader guarantees a room reaches at most half its candidate
//! lattice, so the candidate window for any box is a small constant (at most
//! 3 cells per axis for a section, 2 for a point) regardless of how large the
//! author makes the room.

use crate::data::underground::{Chamber, UndergroundBiomes};
use crate::rng::FeatureRng;

use super::settings::CAVE_LATTICE_STEP;

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
    /// This lobe's own floor, never below the room's. A satellite riding high
    /// in the room therefore floors HIGHER than the primary, and where it
    /// reaches past the primary the floor steps up — a terrace. One sill for
    /// the whole room is a machined disc at a single Y across the entire
    /// footprint, which is the read Rachel objected to in the stamped hall.
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
    /// `rho_s * |u_xz| <= rx` hold for every direction and either aspect, so
    /// the reach is exactly `rx + feather` horizontally and `ry + feather`
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
        let n2 = (dx * dx + dz * dz) / (self.rx * self.rx) + (dy * dy) / (self.ry * self.ry);
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

/// One rolled room, resolved against its row's territory.
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
    /// `(threshold term, tunnel gain term)`. Both are read at every lattice
    /// corner, and both must be EXACTLY `0.0` past the declared reach.
    #[inline]
    fn at(&self, x: i32, y: i32, z: i32, knead: f64) -> (f64, f64) {
        if y < self.sill_y {
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
/// corners. Empty for every table that declares no chamber, which is the
/// shipped one.
pub(super) struct ChamberField {
    rooms: Vec<Room>,
    /// Whether any gathered room widens the radius carvers, so the second lane
    /// is allocated only for tables that ask for it.
    any_tunnel: bool,
}

impl ChamberField {
    #[inline]
    pub(super) fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    #[inline]
    pub(super) fn any_tunnel(&self) -> bool {
        self.any_tunnel
    }

    /// Test seam: where the rolled rooms are, so an adversarial sweep can aim
    /// its boxes at the rims instead of hoping a random one lands on a room.
    #[cfg(test)]
    pub(super) fn centers(&self) -> Vec<[i32; 3]> {
        self.rooms.iter().map(|r| r.center).collect()
    }

    /// `(threshold term, tunnel gain term)` summed over every gathered room, in
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
        table: &UndergroundBiomes,
        seed: u32,
        lo: [i32; 3],
        hi: [i32; 3],
        biome_field_at: impl Fn(i32, i32, i32) -> f64,
    ) -> ChamberField {
        let mut rooms = Vec::new();
        for ch in table.chambers() {
            gather_row(table, ch, seed, lo, hi, &biome_field_at, &mut rooms);
        }
        let any_tunnel = rooms.iter().any(|r| r.tunnel_gain > 0.0);
        ChamberField { rooms, any_tunnel }
    }
}

fn gather_row(
    table: &UndergroundBiomes,
    ch: &Chamber,
    seed: u32,
    lo: [i32; 3],
    hi: [i32; 3],
    biome_field_at: &impl Fn(i32, i32, i32) -> f64,
    out: &mut Vec<Room>,
) {
    let l = ch.lattice;
    let pad = ch.reach_xz();
    let cells = |a: i32, b: i32| (a - pad).div_euclid(l)..=(b + pad).div_euclid(l);
    // Jitter is in whole lattice steps so a centre lands on a cave-lattice
    // corner; see `gather`.
    let steps = l / CAVE_LATTICE_STEP;
    let band = table.row_y(ch.row);

    // Canonical iteration order (see the module docs): ascending absolute
    // column coordinates. Every box covering a given corner walks the rooms
    // that matter there in this same sequence.
    for cz in cells(lo[2], hi[2]) {
        for cx in cells(lo[0], hi[0]) {
            let mut rng = FeatureRng::positional(seed, ch.salt, cx, 0, cz);
            if rng.next_i32(0, ch.one_in - 1) != 0 {
                continue;
            }
            let jitter = |r: &mut FeatureRng| r.next_i32(0, steps - 1) * CAVE_LATTICE_STEP;
            let (jx, jz) = (jitter(&mut rng), jitter(&mut rng));
            let rx = rng.next_i32(ch.r_min, ch.r_max);
            let (mut lobes, n_lobes, ex, ey) = roll_lobes(&mut rng, ch, rx);
            // Depth is rolled inside the row's own band, shrunk by the room's
            // ACTUAL extent, so the room cannot stick out of the territory that
            // authorised it — and snapped to the cave lattice so the biome
            // sample below is exactly the value the carver reads there. The
            // loader guarantees a legal centre exists for the row's worst case,
            // so the rolled-extent window is never empty when that one is not.
            let drop = ch.drop(rx);
            let Some(cy) = roll_center_y(&mut rng, band, drop, ey) else {
                continue;
            };
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
            let room = Room {
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
            };
            // Drop rooms that contribute exactly zero everywhere in the box
            // BEFORE the biome sample: this is the only filter allowed to
            // depend on the box, so it must not change any surviving value.
            if !room.reaches(lo, hi) {
                continue;
            }
            // Territory: a room exists only where its own row WINS the cell its
            // centre sits in — the same resolution lining and caliber go
            // through, so a chamber can never appear in a biome that did not
            // ask for one.
            let f = biome_field_at(center[0], center[1], center[2]);
            if table.id_at(f, center[1]) != ch.row {
                continue;
            }
            out.push(room);
        }
    }
}

/// The primary lobe plus its rolled satellites, and the room's actual
/// half-extents. Satellites are offset in units of the PRIMARY's radii, so the
/// loader can bound the whole cluster from `lobe_spread + lobe_scale[1]` with
/// no per-roll reasoning, and the offsets are drawn per axis so the cluster is
/// never axis-aligned in practice.
fn roll_lobes(rng: &mut FeatureRng, ch: &Chamber, rx: i32) -> ([Lobe; MAX_LOBES], usize, i32, i32) {
    let (rx, ry) = (rx as f64, ch.ry(rx));
    let mut lobes = [Lobe::default(); MAX_LOBES];
    // Sills are filled in once the centre is rolled; see `gather_row`.
    lobes[0] = Lobe {
        off: [0.0; 3],
        rx,
        ry,
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
            ..Lobe::default()
        };
    }
    let ex = lobes[..n]
        .iter()
        .map(|l| (l.off[0].abs().max(l.off[2].abs()) + l.rx + ch.feather).ceil() as i32)
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
mod tests {
    use super::*;
    use crate::data::underground;
    use crate::noise::settings::CAVE_MIN_Y;

    const PACK: &str = r#"{"underground_biomes": [
        {"underground_biome": "mymod:cathedral", "field": [0.24, 1.0], "y": [-64, -16],
         "lining": {"block": "petramond:moss_block", "shell": 1.4},
         "chamber": {"lattice": 64, "one_in": 1, "radius": [10, 14],
                     "flatten": 0.45, "sill": 0.75, "feather": 9, "strength": 1.0,
                     "lobes": 3, "lobe_spread": 0.7, "lobe_scale": [0.55, 0.8],
                     "tunnel": 2.0, "rim_noise": 1.5}}]}"#;

    fn field(
        table: &'static underground::UndergroundBiomes,
        seed: u32,
    ) -> super::super::cave_field::CaveField {
        super::super::cave_field::CaveField::with_table(seed, table)
    }

    /// Every room the seed actually rolls over a wide box — the only honest
    /// population to sweep, since a hand-built `Room` cannot exercise the lobe
    /// roll and a lobe offset missing from a reach bound is a carve seam.
    fn rolled(f: &super::super::cave_field::CaveField) -> Vec<Room> {
        let g = f.chamber_field([-768, -64, -768], [768, -8, 768]);
        g.rooms.clone()
    }

    /// The invariant the whole module is arranged around: a corner's value may
    /// not depend on which box asked for it. A superset gather must add only
    /// exact zeros, and the surviving rooms must accumulate in the same order —
    /// f64 addition is not associative, so "close enough" is a seam. BOTH lanes
    /// are checked: the tunnel gain rides the same accumulation.
    #[test]
    fn a_chamber_corner_does_not_depend_on_the_box_it_was_gathered_for() {
        let table = underground::test_table(&[PACK]);
        for seed in [0x312u32, 0x1D001, 0xBEEF] {
            let f = field(table, seed);
            let step = CAVE_LATTICE_STEP;
            for (lo, hi) in [
                ([0, -48, 0], [16, -32, 16]),
                ([-64, -64, -64], [64, -16, 64]),
                ([-256, -60, 128], [-240, -44, 144]),
            ] {
                let small = f.chamber_field(lo, hi);
                // A box grown by two whole lattice cells of the chamber grid
                // gathers a strict superset of the candidates.
                let big = f.chamber_field(
                    [lo[0] - 128, lo[1] - 128, lo[2] - 128],
                    [hi[0] + 128, hi[1] + 128, hi[2] + 128],
                );
                let mut y = lo[1];
                while y <= hi[1] {
                    let mut z = lo[2];
                    while z <= hi[2] {
                        let mut x = lo[0];
                        while x <= hi[0] {
                            let k = f.knead_sample(x, y, z);
                            let (a, b) = (small.at(x, y, z, k), big.at(x, y, z, k));
                            assert_eq!(
                                (a.0.to_bits(), a.1.to_bits()),
                                (b.0.to_bits(), b.1.to_bits()),
                                "chamber at ({x},{y},{z}) differs by gather box (seed {seed:#x})"
                            );
                            x += step;
                        }
                        z += step;
                    }
                    y += step;
                }
            }
        }
    }

    /// The profile must be EXACTLY zero past the reach every bound is derived
    /// from — the lattice-lane gate, the candidate window and the skip mask all
    /// rest on it — and must saturate somewhere inside, or the room is a sponge
    /// rather than a room.
    ///
    /// Swept over ROLLED rooms rather than a literal, because the reach a lobed
    /// room has to respect is the ROW's (`reach_xz` / `rise`), and only the roll
    /// can put a satellite where that bound is tight.
    #[test]
    fn a_rolled_room_is_exactly_zero_past_its_declared_reach() {
        let table = underground::test_table(&[PACK]);
        let ch = table.chambers()[0];
        let mut rooms = 0usize;
        let mut rims = 0usize;
        for seed in [0x312u32, 0x1D001, 0x2BEEF] {
            let f = field(table, seed);
            for r in rolled(&f) {
                rooms += 1;
                let c = r.center;
                // The kneading field reaches about +-0.54; the reach must hold
                // for every value it can take, including the ones that widen
                // the ramp.
                let kneads = [-0.55, -0.2, 0.0, 0.2, 0.55];
                assert_eq!(
                    r.at(c[0], c[1], c[2], 0.0).0,
                    ch.strength,
                    "the core must saturate"
                );
                for k in kneads {
                    assert_eq!(
                        r.at(c[0], r.sill_y - 1, c[2], k),
                        (0.0, 0.0),
                        "below the sill"
                    );
                    for d in [ch.reach_xz(), ch.reach_xz() + 1, ch.reach_xz() + 97] {
                        for p in [
                            [c[0] + d, c[1], c[2]],
                            [c[0] - d, c[1], c[2]],
                            [c[0], c[1], c[2] + d],
                            [c[0], c[1], c[2] - d],
                        ] {
                            assert_eq!(
                                r.at(p[0], p[1], p[2], k),
                                (0.0, 0.0),
                                "horizontal reach {d} knead {k}"
                            );
                        }
                    }
                    // `rise` is taken over every radius the ROW can roll, so it
                    // bounds this room whatever its lobes came out as.
                    let rise = ch.extent_y().1;
                    for d in [rise, rise + 1, rise + 97] {
                        assert_eq!(
                            r.at(c[0], c[1] + d, c[2], k),
                            (0.0, 0.0),
                            "vertical reach {d} knead {k}"
                        );
                    }
                }
                // and it must actually blend, not step
                for k in 1..ch.feather as i32 {
                    let v = r.at(c[0] + ch.r_min + k, c[1], c[2], 0.0).0;
                    if v > 0.0 && v < ch.strength {
                        rims += 1;
                        break;
                    }
                }
            }
        }
        assert!(
            rooms > 4,
            "only {rooms} rooms rolled; the sweep proves little"
        );
        assert_eq!(rims, rooms, "some room's rim is a step, not a blend");
    }

    /// Three things a room may never do: appear outside the territory its own
    /// row declares (or a pack's cathedral turns up in another pack's biome),
    /// stick out of that row's DEPTH band (or its roof is unlined, undressed,
    /// and impossible to attribute to this file), or reach BELOW the carvable
    /// floor — where it is sliced by bedrock into a dead-flat plane of bare
    /// rock instead of tapering to its own sill. The last one is one `max()`
    /// that a refactor loses in silence.
    #[test]
    fn a_room_stays_inside_its_row_territory_band_and_the_carvable_range() {
        let table = underground::test_table(&[PACK]);
        let ch = table.chambers()[0];
        let band = table.row_y(ch.row);
        let f = field(table, 0x1D001);
        let mut rooms = 0usize;
        for r in rolled(&f) {
            rooms += 1;
            let bf = f.biome_field_sample(r.center);
            assert_eq!(
                table.id_at(bf, r.center[1]),
                ch.row,
                "room at {:?} is not in its own row's territory",
                r.center
            );
            assert!(
                r.sill_y >= CAVE_MIN_Y,
                "room {:?} sills at {} , below the carvable floor {CAVE_MIN_Y}",
                r.center,
                r.sill_y
            );
            assert!(
                r.sill_y >= band.0 && r.center[1] + r.ey <= band.1,
                "room {:?} sticks out of the band {band:?}",
                r.center
            );
            // and the profile agrees with the extents the band test used
            for k in [-0.55, 0.0, 0.55] {
                assert_eq!(r.at(r.center[0], band.0 - 1, r.center[2], k), (0.0, 0.0));
                assert_eq!(r.at(r.center[0], band.1 + 1, r.center[2], k), (0.0, 0.0));
            }
        }
        assert!(rooms > 0, "no room rolled; the test proves nothing");
    }

    /// A satellite lobe that clears the primary entirely leaves a detached
    /// bubble: a sealed pocket, which is the exact failure the attachment work
    /// exists to remove. The loader bounds `lobe_spread` against
    /// `lobe_scale[0]` for that, and this is the roll actually obeying it —
    /// the offsets are drawn per axis, so the diagonal is the case to check.
    #[test]
    fn no_rolled_satellite_lobe_can_detach_from_its_room() {
        let table = underground::test_table(&[PACK]);
        let f = field(table, 0x26AAC);
        let mut satellites = 0usize;
        for r in rolled(&f) {
            let p = r.lobes[0];
            for l in &r.lobes[1..r.n_lobes] {
                satellites += 1;
                // Normalised against the PRIMARY, both lobes are spheres: the
                // satellite is a sphere of radius `l.rx / p.rx` at this
                // distance from the origin.
                let d = ((l.off[0] / p.rx).powi(2)
                    + (l.off[1] / p.ry).powi(2)
                    + (l.off[2] / p.rx).powi(2))
                .sqrt();
                assert!(
                    d < 1.0 + l.rx / p.rx,
                    "satellite at normalised distance {d} with radius {} is detached",
                    l.rx / p.rx
                );
            }
        }
        assert!(
            satellites > 0,
            "no satellite rolled; the test proves nothing"
        );
    }
}
