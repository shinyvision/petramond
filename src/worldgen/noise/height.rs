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

use crate::biome::Climate;
use crate::chunk::CHUNK_SY;
use crate::mathh::smoothstep;
use super::settings::*;

use noise::{
    MultiFractal, NoiseFn, OpenSimplex, Perlin, RidgedMulti, Seedable, Fbm,
};

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
    river: RidgedMulti<Perlin>,  // low-freq ridged -> rivers where ~0
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
            river: RidgedMulti::<Perlin>::default()
                .set_seed(s(SALT_RIVER))
                .set_octaves(RIVER_OCTAVES)
                .set_frequency(RIVER_FREQ),
        }
    }

    // ---- decomposed height helpers (bit-identical to the inline god-file math) ----

    /// Continentalness mapped to a base land height in ~[40, 100]. The `as f32`
    /// inside `smoothstep` is a load-bearing parity cast (see module docs).
    #[inline]
    fn base_height(&self, cont: f64) -> f64 {
        let cont01 = (cont * 0.5 + 0.5).clamp(0.0, 1.0);
        40.0 + 60.0 * smoothstep(0.0, 1.0, cont01 as f32) as f64
    }

    /// Smoothstep gate that lets jagged ridges contribute only on already-high
    /// terrain. The `as f32` cast is load-bearing for parity.
    #[inline]
    fn peak_gate(&self, jag: f64) -> f64 {
        smoothstep(0.55, 0.95, jag as f32) as f64
    }

    /// Climate sample at world (x,z) — produces the 6-parameter tuple.
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let fx = x as f64;
        let fz = z as f64;
        // Temperature: slow latitude-like gradient + noise. Period ~4000.
        let t = self.temperature.get([fx * 0.000_25, fz * 0.000_25]);
        // Humidity: similar low frequency, offset.
        let h = self.humidity.get([fx * 0.000_30, fz * 0.000_30]);
        let c = self.continentalness.get([fx, fz]);
        let e = self.erosion.get([fx * 0.000_45, fz * 0.000_45]);
        let w = self.weirdness.get([fx, fz]);
        let d = self.depth.get([fx * 0.000_60, fz * 0.000_60]);
        Climate {
            temperature: t as f32,
            humidity: h as f32,
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

        // Continentalness (fbm, ~[-1,1]): major land vs ocean shape.
        let cont = self.continentalness.get([fx, fz]);
        let cont01 = (cont * 0.5 + 0.5).clamp(0.0, 1.0);
        let base = self.base_height(cont);

        // Erosion: negative = rugged, positive = smooth/flat.
        let erosion = self.erosion.get([fx * 0.000_45, fz * 0.000_45]);
        let er_factor = (erosion * 0.5 + 0.5).clamp(0.0, 1.0); // 0 rough, 1 smooth

        // Weirdness (fbm, ~[-1,1]): medium-frequency hills and valleys.
        let weird = self.weirdness.get([fx, fz]);
        let hill_amp = (1.0 - 0.5 * er_factor) * (10.0 + 18.0 * cont01);
        let h = base + weird * hill_amp;

        // Jagged ridged noise for sharp peaks, gated to already-elevated terrain.
        let jag = self.jagged.get([fx * 0.012, fz * 0.012]); // ~[0,1]
        let jag_amp = (1.0 - 0.85 * er_factor) * (8.0 + 22.0 * cont01);
        let h = h + jag_amp * self.peak_gate(jag);

        // Surface detail (mid freq) — gentle rolling hills.
        let surf = self.surface.get([fx * 0.018, fz * 0.018]);
        let h = h + surf * 3.0 * (1.0 - 0.5 * er_factor);

        // Micro offset (high freq) — small bumps, capped.
        let off = self.offset.get([fx * 0.08, fz * 0.08]);
        let h = h + off * 1.0;

        let h = h.round() as i32;
        h.clamp(4, CHUNK_SY as i32 - 8)
    }

    /// River intensity at world (x,z): 0 = no river, 1 = carved fully.
    pub fn river_strength(&self, x: i32, z: i32) -> f32 {
        let fx = x as f64;
        let fz = z as f64;
        // Ridged noise produces ridge lines in [0,1]; rivers sit *on* ridges
        // (near 0 in RidgedMultifractal's "valley", since inverse of ridges).
        let r = self.river.get([fx * 0.001_6, fz * 0.001_6]); // [0,1]
        // Carve where the value is near 0 (between ridges), gated by a low-freq
        // mask so rivers don't cover the whole world.
        let mask = self.depth.get([fx * 0.000_5, fz * 0.000_5]) as f32 * 0.5 + 0.5;
        let in_channel = (1.0 - (r as f32 - 0.0).abs().min(1.0)).powi(2);
        let strength = (in_channel - 0.85).max(0.0) / 0.15; // sharp band
        strength * smoothstep(0.4, 0.9, mask)
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
