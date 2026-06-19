//! Typed height / climate / river field — the numeric core of worldgen.
//!
//! Strata P1: extracted from `gen.rs::WorldNoise`. The math is **byte-identical**
//! to the god file (locked by the genparity gate); it is only decomposed into
//! named, unit-testable helpers. We deliberately keep a plain typed function
//! rather than a runtime density-function interpreter: terrain here is a 2-D
//! per-column height field (~40 lines of math), so a `Box<dyn>`/enum DAG would
//! add a per-node interpreter and — most dangerously — an opportunity for the
//! f64->f32 cast points below to silently drift.
//!
//! The `as f32` casts inside `base_height` and `peak_gate` are LOAD-BEARING for
//! parity (`mathh::smoothstep` is f32). Do not "clean them up" to all-f64.

use super::settings::*;
use crate::biome::Climate;
use crate::chunk::CHUNK_SY;
use crate::mathh::smoothstep;

use noise::{Fbm, MultiFractal, NoiseFn, OpenSimplex, Perlin, RidgedMulti, Seedable};

/// Owns the named noise samplers and computes per-column surface height,
/// climate, and river strength. Immutable after construction; `Send + Sync`.
pub struct HeightField {
    temperature: OpenSimplex,
    humidity: OpenSimplex,
    continentalness: Fbm<OpenSimplex>,
    erosion: OpenSimplex,
    weirdness: Fbm<OpenSimplex>,
    depth: OpenSimplex,
    jagged: RidgedMulti<Perlin>, // sharp ridges for peaks
    surface: Perlin,             // high-freq surface detail
    offset: Perlin,              // micro surface noise
    river: Fbm<OpenSimplex>,     // smooth fbm; rivers run along its zero-contour
    river_warp: OpenSimplex,     // domain warp (+ width modulation) for river meander
    density3d: OpenSimplex,      // 3-D overhang carve (sampled per band voxel)
    pv: Fbm<OpenSimplex>,        // peaks & valleys: broad mountain massing
    crag: RidgedMulti<Perlin>,   // broad craggy ridgelines on peaks (walkable, not pillars)
}

impl HeightField {
    pub fn new(seed: u32) -> Self {
        let s = |salt: u32| seed.wrapping_add(salt);
        Self {
            temperature: OpenSimplex::new(s(SALT_TEMP)),
            humidity: OpenSimplex::new(s(SALT_HUMID)),
            continentalness: Fbm::<OpenSimplex>::new(s(SALT_CONT))
                .set_octaves(CONT_OCTAVES)
                .set_frequency(CONT_FREQ),
            erosion: OpenSimplex::new(s(SALT_EROS)),
            weirdness: Fbm::<OpenSimplex>::new(s(SALT_WEIRD))
                .set_octaves(WEIRD_OCTAVES)
                .set_frequency(WEIRD_FREQ),
            depth: OpenSimplex::new(s(SALT_DEPTH)),
            jagged: RidgedMulti::<Perlin>::default()
                .set_seed(s(SALT_JAG))
                .set_octaves(JAG_OCTAVES)
                .set_frequency(JAG_FREQ),
            surface: Perlin::new(s(SALT_SURF)),
            offset: Perlin::new(s(SALT_OFF)),
            river: Fbm::<OpenSimplex>::new(s(SALT_RIVER))
                .set_octaves(RIVER_OCTAVES)
                .set_frequency(RIVER_FREQ),
            river_warp: OpenSimplex::new(s(SALT_RIVERW)),
            density3d: OpenSimplex::new(s(SALT_DENS3D)),
            pv: Fbm::<OpenSimplex>::new(s(SALT_PV))
                .set_octaves(PV_OCTAVES)
                .set_frequency(PV_FREQ),
            crag: RidgedMulti::<Perlin>::default()
                .set_seed(s(SALT_CRAG))
                .set_octaves(CRAG_OCTAVES)
                .set_frequency(CRAG_FREQ),
        }
    }

    /// Debug: raw noise field samples at a column — (cont, erosion, pv, jagged).
    /// Used by the genmap `stats` mode to calibrate amplitudes against the actual
    /// (non-normalised) ranges the `noise` crate produces.
    pub fn debug_sample(&self, x: i32, z: i32) -> (f64, f64, f64, f64) {
        let (fx, fz) = (x as f64, z as f64);
        (
            self.continentalness.get([fx, fz]),
            self.erosion.get([fx * 0.000_80, fz * 0.000_80]),
            self.pv.get([fx, fz]),
            self.jagged.get([fx * 0.012, fz * 0.012]),
        )
    }

    /// 3-D overhang noise at a world voxel, in ~[-1, 1]. Anisotropic: finer in Y
    /// so the warped surface leans and undercuts. Sampled only inside the carve
    /// band of mountain columns (see the driver's per-column precompute).
    #[inline]
    pub fn overhang_noise(&self, x: i32, y: i32, z: i32) -> f64 {
        // Raw OpenSimplex only spans ~[-0.3,0.3]; expand so the warped surface has
        // a steep enough vertical slope to actually fold back into overhangs.
        let n = self.density3d.get([
            x as f64 * DENS3D_FREQ_XZ,
            y as f64 * DENS3D_FREQ_Y,
            z as f64 * DENS3D_FREQ_XZ,
        ]);
        (n * 3.2).clamp(-1.0, 1.0)
    }

    // ---- decomposed height helpers (bit-identical to the inline god-file math) ----

    /// Continentalness (0..1) mapped to a base floor height via a monotone
    /// piecewise-linear spline: a deep ocean basin well below sea level, a coastal
    /// shelf around sea level (so beaches form), then rising lowland, foothill
    /// shoulder, and mountain base. Pure f64 (monotone, no f32 cast needed).
    #[inline]
    fn base_floor(&self, c: f64) -> f64 {
        // (cont01, floor_y) control points — monotone increasing.
        const PTS: [(f64, f64); 8] = [
            (0.00, 24.0),  // deep ocean basin (~40 blocks of water under sea=64)
            (0.18, 38.0),  // deep ocean
            (0.34, 52.0),  // shallow ocean shelf
            (0.44, 60.0),  // coastal shelf (just under sea)
            (0.49, 70.0),  // STEEP shore crossing (thin beaches, not wide flats)
            (0.60, 76.0),  // plains / forest lowland
            (0.78, 90.0),  // foothill shoulder
            (1.00, 104.0), // mountain base (peaks ride on top)
        ];
        let mut i = 0;
        while i < PTS.len() - 1 && c > PTS[i + 1].0 {
            i += 1;
        }
        let (c0, y0) = PTS[i];
        let (c1, y1) = PTS[(i + 1).min(PTS.len() - 1)];
        let t = if c1 > c0 {
            ((c - c0) / (c1 - c0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        y0 + (y1 - y0) * t
    }

    /// Climate sample at world (x,z) — produces the 6-parameter tuple.
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let fx = x as f64;
        let fz = z as f64;
        // Climate frequencies raised ~10x vs v1 so biomes actually vary across a
        // few-hundred-block scale instead of one cell smearing the whole world.
        // Continentalness + weirdness stay broad (coherent coastlines + edge dither).
        let t = self.temperature.get([fx * 0.0024, fz * 0.0024]); // period ~420
        let h = self.humidity.get([fx * 0.0028, fz * 0.0028]); // period ~360
        let c = self.continentalness.get([fx, fz]);
        let e = self.erosion.get([fx * 0.0016, fz * 0.0016]);
        let w = self.weirdness.get([fx, fz]);
        let d = self.depth.get([fx * 0.0020, fz * 0.0020]);
        // Raw temp/humid only span ~[-0.5,0.5]; expand so the hot/dry (desert) and
        // very-humid (swamp) corners of the biome grid are actually reachable.
        Climate {
            temperature: (t * 2.0).clamp(-1.0, 1.0) as f32,
            humidity: (h * 2.1).clamp(-1.0, 1.0) as f32,
            continentalness: c as f32,
            erosion: e as f32,
            weirdness: w as f32,
            depth: d as f32,
        }
    }

    /// Surface height (top solid block Y) at world (x,z).
    /// Byte-for-byte identical to `gen.rs::WorldNoise::surface_height`.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let fx = x as f64;
        let fz = z as f64;

        // Continentalness (fbm): ocean vs land. The raw fbm only spans ~[-0.3,0.3],
        // so expand ~2.8x to actually reach the deep-ocean and mountain-base ends
        // of the base-floor spline.
        let cont = self.continentalness.get([fx, fz]);
        let cont_x = (cont * 2.8).clamp(-1.0, 1.0);
        let cont01 = (cont_x * 0.5 + 0.5).clamp(0.0, 1.0);

        // Erosion (low = rugged mountains, high = smooth lowland). Sampled at a
        // mid frequency so mountain ranges are coherent bands, not random spikes.
        let erosion = self.erosion.get([fx * 0.000_80, fz * 0.000_80]);
        let er01 = (erosion * 0.5 + 0.5).clamp(0.0, 1.0); // 0 rugged .. 1 smooth
        let rugged = 1.0 - er01;

        // (A) Base floor from continentalness: deep basin -> coast -> highland base.
        let base = self.base_floor(cont01);

        // Domain warp: displace the mountain-mass + crag sample points by a low-
        // frequency field so massifs become bent, irregular ridge systems instead
        // of radially-symmetric smooth domes (the "boobs" failure mode).
        let warp_x = self.surface.get([fx * WARP_FREQ, fz * WARP_FREQ]) * WARP_AMP;
        let warp_z = self
            .offset
            .get([fx * WARP_FREQ + 19.3, fz * WARP_FREQ + 4.1])
            * WARP_AMP;
        let wx = fx + warp_x;
        let wz = fz + warp_z;

        // (B) Mountain massif: placed by the WARPED peaks-&-valleys fbm so ranges
        // recur in space; erosion sets how tall/rugged they grow. Only ever ADDS
        // height above the base. A narrow smoothstep + power gives a STEEP shoulder
        // so ranges rise abruptly from the lowland instead of swelling gently.
        // `peak` is a column's 0..1 "mountain-ness".
        let inland = smoothstep(0.30, 0.52, cont01 as f32) as f64; // 0 ocean/coast .. 1 land
        let pv = self.pv.get([wx, wz]); // raw ~[-0.23,0.35], period ~550
        let pv01 = (pv * 3.4 * 0.5 + 0.5).clamp(0.0, 1.0); // expand to span [0,1]
        let peak = (smoothstep(0.56, 0.90, pv01 as f32) as f64).powf(1.3) * inland;
        let h = base + peak * (28.0 + 80.0 * rugged); // taller where rugged

        // (C) Rolling hills everywhere (gentle), damped hard on mountain faces.
        let weird = self.weirdness.get([fx, fz]);
        let hill_amp = (1.0 - 0.5 * er01) * (5.0 + 13.0 * cont01) * (1.0 - 0.7 * peak);
        let h = h + weird * hill_amp;

        // (D) CRAGGINESS — broad ridged relief that gives a mountain craggy
        // character without turning it into a field of 1-wide pillars. `crag` is a
        // deliberately BROAD ridged stack (2 octaves, ~77-block period) sampled on
        // the WARPED coords. RidgedMulti output is ~[-1, 1.3]; we CENTRE it (×0.45)
        // so the surface undulates over walkable slopes instead of spiking up from
        // a flat floor, and the amplitude is modest. Gated by `peak` so lowlands
        // stay flat. (Earlier high-freq/high-amp versions made unwalkable spikes —
        // verify walkability with `genmap … rough`, not just a cross-section.)
        let ridge = self.crag.get([wx, wz]);
        let crag = (ridge * 0.45 + 0.18).clamp(-0.35, 0.95);
        let jag_amp = 8.0 + 16.0 * rugged;
        let h = h + peak * jag_amp * crag;

        // (E) Surface detail (mid freq) — gentle texture.
        let surf = self.surface.get([fx * 0.018, fz * 0.018]);
        let h = h + surf * 2.5 * (1.0 - 0.5 * er01);

        // (F) Micro offset (high freq) — small bumps.
        let off = self.offset.get([fx * 0.08, fz * 0.08]);
        let h = h + off * 1.0;

        let h = h.round() as i32;
        h.clamp(4, CHUNK_SY as i32 - 8)
    }

    /// River intensity at world (x,z): 0 = no river, 1 = channel centre.
    ///
    /// Rivers run along the ZERO-CONTOUR of a smooth fbm: where `|n|` is small the
    /// column is on the river, ramping linearly to 0 at the bank. A smooth noise's
    /// zero set is a network of long winding curves, so this gives connected
    /// meandering rivers. (The previous version thresholded a `RidgedMulti` near 0,
    /// but ridged output never approaches 0 — and the sample coords were scaled a
    /// second time on top of `set_frequency`, pinning the field to a near-constant
    /// ~0.83 — so `river_strength` was identically 0 and nothing ever carved.)
    ///
    /// Frequency is LITERAL: sampled with raw world coords, so `RIVER_FREQ` is the
    /// real period — no second coordinate multiplier (the double-scaling trap).
    ///
    /// Width is measured in BLOCKS: the distance to the centreline is `|n|` divided
    /// by the local gradient, so a river is the same width whether the noise is
    /// locally steep or flat (a raw `|n|` band balloons over flat stretches into
    /// huge blobs). Near a local extremum that never crosses zero the gradient is
    /// tiny, so the distance blows up and no false river/lake forms there.
    ///
    /// The sample point is domain-warped first so the channel meanders instead of
    /// drawing clean geometric arcs, and the half-width is modulated along the
    /// course so the river pinches and widens.
    pub fn river_strength(&self, x: i32, z: i32) -> f32 {
        let fx = x as f64;
        let fz = z as f64;
        // Domain warp: displace the sample point by a medium-frequency field. The
        // offset is treated as locally constant, so the gradient (and thus width)
        // is still measured correctly in world blocks at the warped location.
        let dx = self
            .river_warp
            .get([fx * RIVER_WARP_FREQ, fz * RIVER_WARP_FREQ])
            * RIVER_WARP_AMP;
        let dz = self
            .river_warp
            .get([fx * RIVER_WARP_FREQ + 31.7, fz * RIVER_WARP_FREQ + 5.1])
            * RIVER_WARP_AMP;
        let (sx, sz) = (fx + dx, fz + dz);
        let n = self.river.get([sx, sz]);
        // Gradient over a ±2-block stencil rather than ±1: the wider stencil
        // low-passes the gradient estimate so `dist` (and thus the carved floor)
        // doesn't swing sharply between adjacent columns near low-gradient fade
        // points — those swings were the source of small bank stairs.
        let gx = (self.river.get([sx + 2.0, sz]) - self.river.get([sx - 2.0, sz])) * 0.25;
        let gz = (self.river.get([sx, sz + 2.0]) - self.river.get([sx, sz - 2.0])) * 0.25;
        let grad = (gx * gx + gz * gz).sqrt().max(1e-6);
        let dist = (n.abs() / grad) as f32; // blocks from the channel centreline
                                            // Vary width along the course (broad, decorrelated low-freq field).
        let wmod = self
            .river_warp
            .get([fx * 0.006 + 100.0, fz * 0.006 + 100.0]) as f32;
        let half = (RIVER_HALF * (1.0 + RIVER_WIDTH_VAR * wmod)).max(2.0);
        (1.0 - dist / half).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_height_is_deterministic_and_bounded() {
        let f = HeightField::new(0x1234_5678);
        for z in -40..40 {
            for x in -40..40 {
                let a = f.surface_height(x, z);
                assert_eq!(a, f.surface_height(x, z), "non-deterministic at {x},{z}");
                assert!(
                    (4..=(CHUNK_SY as i32 - 8)).contains(&a),
                    "height {a} out of clamp at {x},{z}"
                );
            }
        }
    }

    #[test]
    fn worldnoise_shim_agrees_with_field() {
        // The WorldNoise facade must delegate to HeightField, not diverge.
        let seed = 1u32;
        let f = HeightField::new(seed);
        let wn = super::super::WorldNoise::new(seed);
        for &(x, z) in &[(0, 0), (13, -7), (-100, 250), (999, -999)] {
            assert_eq!(wn.surface_height(x, z), f.surface_height(x, z));
            assert_eq!(wn.river_strength(x, z), f.river_strength(x, z));
            let a = wn.climate(x, z);
            let b = f.climate(x, z);
            assert_eq!(a.temperature, b.temperature);
            assert_eq!(a.humidity, b.humidity);
            assert_eq!(a.continentalness, b.continentalness);
            assert_eq!(a.erosion, b.erosion);
            assert_eq!(a.weirdness, b.weirdness);
            assert_eq!(a.depth, b.depth);
        }
    }
}
