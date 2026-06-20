//! Typed height / climate / river field — the numeric core of worldgen.
//!
//! Strata P1: extracted from the original `WorldNoise` god file into named,
//! unit-testable helpers. We deliberately keep a plain typed function rather
//! than a runtime density-function interpreter: terrain here is a 2-D per-column
//! height field, so a `Box<dyn>`/enum DAG would add a per-node interpreter and —
//! most dangerously — an opportunity for the f64->f32 cast points below to
//! silently drift.
//!
//! The `as f32` casts at `mathh::smoothstep` call sites are intentional because
//! that helper is f32-based. Do not "clean them up" to all-f64 unless the helper
//! changes too.

use super::settings::*;
use super::spline::{erosion_amp, erosion_relief_gain, lift_ceiling, pv_fold};
use crate::biome::{landform_weights, Climate};
use crate::chunk::{CHUNK_SY, SEA_LEVEL};
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
    cave_a: OpenSimplex,         // spaghetti tunnel field A
    cave_b: OpenSimplex,         // spaghetti tunnel field B
    cave_c: OpenSimplex,         // cheese cavern field
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
            cave_a: OpenSimplex::new(s(SALT_CAVE_A)),
            cave_b: OpenSimplex::new(s(SALT_CAVE_B)),
            cave_c: OpenSimplex::new(s(SALT_CAVE_C)),
        }
    }

    /// Should the solid voxel at world `(x, y, z)` be carved to air (a cave)?
    /// Pure function of world position, so caves are identical from every chunk
    /// that touches them — seamless tunnels with no inter-chunk state. The caller
    /// restricts the Y band (keep a floor + solid rock under the surface skin).
    #[inline]
    pub fn cave_carved(&self, x: i32, y: i32, z: i32) -> bool {
        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        // Spaghetti: both decorrelated fields near zero -> a winding tunnel.
        let a = self
            .cave_a
            .get([fx * CAVE_FREQ_XZ, fy * CAVE_FREQ_Y, fz * CAVE_FREQ_XZ]);
        if a.abs() < CAVE_TUNNEL_R {
            let b = self.cave_b.get([
                fx * CAVE_FREQ_XZ + 13.7,
                fy * CAVE_FREQ_Y + 5.1,
                fz * CAVE_FREQ_XZ - 7.3,
            ]);
            if b.abs() < CAVE_TUNNEL_R {
                return true;
            }
        }
        // Cheese: a low-frequency field dipping low -> occasional large caverns.
        let cheese = self.cave_c.get([
            fx * CAVE_CHEESE_FREQ,
            fy * CAVE_CHEESE_FREQ * 1.4,
            fz * CAVE_CHEESE_FREQ,
        ]);
        cheese < CAVE_CHEESE_T
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

    /// Debug: raw weirdness sample at a column. Kept separate from
    /// `debug_sample` so that facade stays ABI-stable for local tools.
    pub fn debug_weirdness(&self, x: i32, z: i32) -> f64 {
        self.weirdness.get([x as f64, z as f64])
    }

    /// Debug: shared landform weights (mountain, foothill, rolling, plateau,
    /// wet_basin) used by biome selection and surface shaping.
    pub fn debug_landform(&self, x: i32, z: i32) -> (f32, f32, f32, f32, f32) {
        let land = landform_weights(self.climate(x, z));
        (
            land.mountain,
            land.foothill,
            land.rolling,
            land.plateau,
            land.wet_basin,
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

    // ---- decomposed height helpers ----

    /// Continentalness (0..1) mapped to a base floor height via a monotone
    /// piecewise-linear spline: a deep ocean basin well below sea level, a coastal
    /// shelf around sea level (so beaches form), then rising lowland, foothill
    /// shoulder, and mountain base. Pure f64 (monotone, no f32 cast needed).
    #[inline]
    fn base_floor(&self, c: f64) -> f64 {
        // (cont01, floor_y) control points — monotone increasing.
        const PTS: [(f64, f64); 8] = [
            (0.00, 24.0), // deep ocean basin (~40 blocks of water under sea=64)
            (0.18, 38.0), // deep ocean
            (0.34, 52.0), // shallow ocean shelf
            (0.44, 60.0), // coastal shelf (just under sea)
            (0.49, 70.0), // STEEP shore crossing (thin beaches, not wide flats)
            (0.62, 75.0), // plains / forest lowland
            (0.82, 82.0), // rolling upland
            (1.00, 88.0), // high plateau base (peaks ride on top)
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

    #[inline]
    fn climate_from_raw(
        temperature: f64,
        humidity: f64,
        continentalness: f64,
        erosion: f64,
        weirdness: f64,
        depth: f64,
    ) -> Climate {
        // Raw temp/humid only span ~[-0.5,0.5]; expand so the hot/dry (desert) and
        // very-humid (swamp) corners of the biome grid are reachable.
        Climate {
            temperature: (temperature * 2.0).clamp(-1.0, 1.0) as f32,
            humidity: (humidity * 2.1).clamp(-1.0, 1.0) as f32,
            continentalness: continentalness as f32,
            erosion: erosion as f32,
            weirdness: weirdness as f32,
            depth: depth as f32,
        }
    }

    #[inline]
    fn climate_raw(&self, fx: f64, fz: f64) -> (f64, f64, f64, f64, f64, f64) {
        (
            self.temperature
                .get([fx * TEMP_SAMPLE_FREQ, fz * TEMP_SAMPLE_FREQ]),
            self.humidity
                .get([fx * HUMID_SAMPLE_FREQ, fz * HUMID_SAMPLE_FREQ]),
            self.continentalness.get([fx, fz]),
            self.erosion
                .get([fx * EROSION_SAMPLE_FREQ, fz * EROSION_SAMPLE_FREQ]),
            self.weirdness.get([fx, fz]),
            self.depth
                .get([fx * DEPTH_SAMPLE_FREQ, fz * DEPTH_SAMPLE_FREQ]),
        )
    }

    /// Climate sample at world (x,z) — produces the 6-parameter tuple.
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let fx = x as f64;
        let fz = z as f64;
        let (t, h, c, e, w, d) = self.climate_raw(fx, fz);
        Self::climate_from_raw(t, h, c, e, w, d)
    }

    /// Surface height (top solid block Y) at world (x,z).
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let fx = x as f64;
        let fz = z as f64;

        let (temp, humid, cont, erosion, weird, depth) = self.climate_raw(fx, fz);
        let climate = Self::climate_from_raw(temp, humid, cont, erosion, weird, depth);
        let land = landform_weights(climate);

        // (A) Continentalness: ocean -> coast -> inland base floor. The raw fbm
        // only spans ~[-0.3,0.3]; expand it to reach both ends of the base-floor
        // spline. Mountains are NOT baked in here — they emerge below from the
        // cont x erosion x PV interaction, so lowland biomes keep their identity
        // on high ground.
        let cont_x = (cont * 2.8).clamp(-1.0, 1.0);
        let cont01 = (cont_x * 0.5 + 0.5).clamp(0.0, 1.0);
        let base = self.base_floor(cont01);

        // (B) Erosion is the MASTER amplitude control. The raw field is narrow, so
        // expand it locally to span the full [0,1]: a single noise then yields both
        // true-flat (er01->1) and fully-rugged (er01->0) provinces — the flat<->
        // steep CONTRAST the old additive stack never had (every region used to get
        // the same mid amplitude). `amp` scales all relief; `gain` fattens the
        // low-erosion uplift (offset/factor splines, see `spline.rs`).
        let er01 = ((erosion * 2.6).clamp(-1.0, 1.0) * 0.5 + 0.5).clamp(0.0, 1.0);
        let amp = erosion_amp(er01);
        let gain = erosion_relief_gain(er01);
        let (weird_pos, weird_neg, strange) = weirdness_shape_weights(weird);

        // Domain warp: bend ridge systems off the noise's symmetry axes so massifs
        // are irregular chains, not radial domes. Weird regions warp harder.
        let warp_amp = WARP_AMP * (1.0 + 0.35 * strange);
        let warp_x = self.surface.get([fx * WARP_FREQ, fz * WARP_FREQ]) * warp_amp;
        let warp_z = self
            .offset
            .get([fx * WARP_FREQ + 19.3, fz * WARP_FREQ + 4.1])
            * warp_amp;
        let wx = fx + warp_x;
        let wz = fz + warp_z;

        // (C) Peaks & valleys: fold a smooth massif noise into ridge/valley
        // structure that shapes EVERY inland column, not just mountains. Plains
        // (high erosion) get gentle swells; low-erosion country gets full
        // ridgelines and carved valleys. Valleys are made shallower than ridges
        // are tall, so low-erosion troughs don't gouge deep flooding gashes.
        let pv_raw = self.pv.get([wx, wz]); // raw ~[-0.23,0.35], period ~550
        let pv_signed = (pv_raw * 3.4).clamp(-1.0, 1.0);
        let pvf = pv_fold(pv_signed); // [-1,1]: -1 valley floor, +1 ridge crest
        let inland = smoothstep(0.46, 0.64, cont01 as f32) as f64;

        // (D) Base relief: base(cont) + emergent uplift + PV relief. Mountains are
        // emergent (high cont x low erosion x ridge), not a landform bolt-on;
        // `land.mountain` now only labels biomes downstream, never gates height.
        let uplift = lift_ceiling(cont01) * (0.35 + 0.65 * (pvf * 0.5 + 0.5)) * amp * gain;
        let pv_relief = if pvf >= 0.0 {
            pvf * inland * (6.0 + 60.0 * amp) // ridges: full height
        } else {
            pvf * inland * (4.0 + 16.0 * amp) // valleys: shallower (anti-gash)
        };
        let h = base + uplift + pv_relief;

        // (E) Inland wet basins. Humid, smooth lowland depressions settle near the
        // waterline even when continentalness says inland — swamps away from oceans
        // without turning them into ocean biomes.
        let basin = land.wet_basin as f64;
        let basin_target = SEA_LEVEL as f64 + 2.0 + 4.0 * (1.0 - basin) + 2.0 * (1.0 - er01);
        let h = h + (basin_target - h) * (0.86 * basin).clamp(0.0, 0.86);

        // (F) CRAGGINESS — knife-edge relief ONLY on ridge crests of low-erosion
        // terrain (and more on negative weirdness). Lowlands and any high-erosion
        // ground get ZERO jaggedness so they stay walkable — spraying spikes
        // everywhere is the classic height-field failure. `crag` is a deliberately
        // BROAD ridged stack centred (×0.45) onto walkable slopes. (Verify
        // walkability with `genmap … rough`, not just a cross-section.)
        let ridge = self.crag.get([wx, wz]);
        let crag = (ridge * 0.45 + 0.18).clamp(-0.35, 0.95);
        let jag_gate = (smoothstep(0.45, 1.0, pvf as f32) as f64)
            * (1.0 - er01).powf(1.5)
            * (0.5 + 0.5 * weird_neg);
        let h = h + jag_gate * (10.0 + 22.0 * (1.0 - er01)) * crag;

        // (G) Weird mountain morphology. Positive weirdness grows extra knife-edge
        // crests on ridges; negative weirdness terraces high faces into shelves.
        // Both gated to low-erosion ridge crests so calm lowlands stay smooth.
        let spine = self.jagged.get([wx * 0.42 + 11.0, wz * 0.42 - 7.0]);
        let spine = (spine * 0.38 + 0.08).clamp(-0.22, 0.92);
        let spine_gate =
            (smoothstep(0.5, 1.0, pvf as f32) as f64) * (1.0 - er01).powf(1.3) * weird_pos;
        let h = h + spine_gate * (12.0 + 20.0 * (1.0 - er01)) * spine;

        let highland = smoothstep(88.0, 116.0, h as f32) as f64;
        let shelf_gate = (0.30 * highland + 0.62 * (smoothstep(0.40, 1.0, pvf as f32) as f64))
            * (1.0 - er01).powf(0.7)
            * weird_neg;
        let shelf_noise = self.surface.get([wx * 0.006 + 37.0, wz * 0.006 - 29.0]);
        let shelf_strength = shelf_gate * (0.24 + 0.22 * (shelf_noise * 0.5 + 0.5));
        let shelf_size = 4.0 + 3.0 * er01 + 2.0 * weird_neg;
        let terraced = (h / shelf_size).round() * shelf_size;
        let h = h + (terraced - h) * shelf_strength;

        // (H) Surface detail (mid freq), damped on flat (high-erosion) ground so
        // plains read genuinely smooth.
        let surf = self.surface.get([fx * 0.018, fz * 0.018]);
        let h = h + surf * 2.5 * amp.max(0.3);

        // (I) Micro offset (high freq) — small bumps.
        let off = self.offset.get([fx * 0.08, fz * 0.08]);
        let h = h + off * 1.0;

        // (J) River valley. Pull the surface DOWN into a continuous low valley
        // wherever a river runs, so the channel floods as one connected, properly
        // wide water course regardless of the surrounding terrain height — rivers
        // are low ground (like a river biome), not a thin cut that dies at every
        // rise. This is the fix for "1-wide rivers that end as soon as height
        // varies": the valley keeps the river low through rolling lowland, and the
        // carver then just shapes the banks. The valley floor is bounded ~16 below
        // the natural surface, so a river crossing genuine highland becomes a
        // shallow dry valley rather than an unnatural slot canyon through a peak.
        let river = self.river_strength(x, z);
        let h = if river > 0.0 {
            let valley = smoothstep(0.04, 0.50, river) as f64; // 0 bank .. 1 core
            let target = (SEA_LEVEL as f64 - 3.0).max(h - 16.0);
            h + (target - h) * valley
        } else {
            h
        };

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

#[inline]
pub(crate) fn weirdness_shape_weights(weird: f64) -> (f64, f64, f64) {
    let weird_pos = (weird * 2.6).clamp(0.0, 1.0);
    let weird_neg = (-weird * 2.6).clamp(0.0, 1.0);
    let strange = (weird.abs() * 2.4).clamp(0.0, 1.0);
    (weird_pos, weird_neg, strange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::{biome_at, Biome};

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

    #[test]
    fn weirdness_shape_weights_track_sign_and_magnitude() {
        assert_eq!(weirdness_shape_weights(0.0), (0.0, 0.0, 0.0));

        let (pos, neg, strange) = weirdness_shape_weights(0.25);
        assert!(pos > 0.0, "positive weirdness should enable ridge crests");
        assert_eq!(neg, 0.0);
        assert!(strange > 0.0);

        let (pos, neg, strange) = weirdness_shape_weights(-0.25);
        assert_eq!(pos, 0.0);
        assert!(neg > 0.0, "negative weirdness should enable terraces");
        assert!(strange > 0.0);

        assert_eq!(weirdness_shape_weights(10.0), (1.0, 0.0, 1.0));
        assert_eq!(weirdness_shape_weights(-10.0), (0.0, 1.0, 1.0));
    }

    #[test]
    fn biome_field_keeps_regions_varied_with_large_deserts_and_inland_swamps() {
        let seed = 42;
        let f = HeightField::new(seed);
        const STEP: i32 = 8;
        const R: i32 = 640;
        let n = (R * 2 / STEP + 1) as usize;
        let mut grid = vec![Biome::Ocean; n * n];
        let mut max_desert_y = i32::MIN;

        for gz in 0..n {
            let wz = -R + gz as i32 * STEP;
            for gx in 0..n {
                let wx = -R + gx as i32 * STEP;
                let climate = f.climate(wx, wz);
                let surf = f.surface_height(wx, wz);
                let biome = biome_at(climate, surf);
                if biome == Biome::Desert {
                    max_desert_y = max_desert_y.max(surf);
                }
                grid[gz * n + gx] = biome;
            }
        }

        let largest_desert = largest_component(&grid, n, Biome::Desert);
        let largest_forest = largest_component(&grid, n, Biome::Forest);
        let largest_savanna = largest_component(&grid, n, Biome::Savanna);
        let largest_snowy_tundra = largest_component(&grid, n, Biome::SnowyTundra);
        let largest_snowy_taiga = largest_component(&grid, n, Biome::SnowyTaiga);
        assert!(
            largest_desert >= 220,
            "largest sampled desert component was {largest_desert} cells"
        );
        assert!(
            largest_forest <= 2_200,
            "largest sampled forest component was {largest_forest} cells"
        );
        assert!(
            largest_savanna <= 2_200,
            "largest sampled savanna component was {largest_savanna} cells"
        );
        assert!(
            largest_snowy_tundra <= 2_600,
            "largest sampled snowy tundra component was {largest_snowy_tundra} cells"
        );
        assert!(
            largest_snowy_taiga <= 2_600,
            "largest sampled snowy taiga component was {largest_snowy_taiga} cells"
        );
        assert!(
            max_desert_y >= 82,
            "expected desert hills/plateaus, max desert surface was y{max_desert_y}"
        );
        assert!(
            has_inland_swamp(&grid, n, 8),
            "expected at least one swamp sample >=64 blocks from sampled ocean"
        );
    }

    fn largest_component(grid: &[Biome], n: usize, target: Biome) -> usize {
        let mut seen = vec![false; grid.len()];
        let mut best = 0usize;
        let mut stack = Vec::new();
        for i in 0..grid.len() {
            if seen[i] || grid[i] != target {
                continue;
            }
            seen[i] = true;
            stack.push(i);
            let mut size = 0usize;
            while let Some(cur) = stack.pop() {
                size += 1;
                let x = cur % n;
                let z = cur / n;
                let push = |nx: usize, nz: usize, seen: &mut [bool], stack: &mut Vec<usize>| {
                    let ni = nz * n + nx;
                    if !seen[ni] && grid[ni] == target {
                        seen[ni] = true;
                        stack.push(ni);
                    }
                };
                if x > 0 {
                    push(x - 1, z, &mut seen, &mut stack);
                }
                if x + 1 < n {
                    push(x + 1, z, &mut seen, &mut stack);
                }
                if z > 0 {
                    push(x, z - 1, &mut seen, &mut stack);
                }
                if z + 1 < n {
                    push(x, z + 1, &mut seen, &mut stack);
                }
            }
            best = best.max(size);
        }
        best
    }

    fn has_inland_swamp(grid: &[Biome], n: usize, radius_cells: usize) -> bool {
        for z in 0..n {
            for x in 0..n {
                if grid[z * n + x] != Biome::Swamp {
                    continue;
                }
                let z0 = z.saturating_sub(radius_cells);
                let z1 = (z + radius_cells).min(n - 1);
                let x0 = x.saturating_sub(radius_cells);
                let x1 = (x + radius_cells).min(n - 1);
                let mut near_ocean = false;
                'scan: for nz in z0..=z1 {
                    for nx in x0..=x1 {
                        if matches!(grid[nz * n + nx], Biome::Ocean | Biome::DeepOcean) {
                            near_ocean = true;
                            break 'scan;
                        }
                    }
                }
                if !near_ocean {
                    return true;
                }
            }
        }
        false
    }
}
