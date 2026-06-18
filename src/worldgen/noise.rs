//! Multi-noise world sampler.
//!
//! Strata P0: relocated verbatim from `gen.rs` (the `WorldNoise` struct + impl).
//! Behavior is byte-identical to the pre-refactor god file; P1 decomposes the
//! height/climate/river math into a typed `HeightField` behind this same ABI.

use crate::biome::Climate;
use crate::chunk::CHUNK_SY;
use crate::mathh::smoothstep;

use noise::{
    MultiFractal, NoiseFn, OpenSimplex, Perlin, RidgedMulti, Seedable, Fbm,
};

pub struct WorldNoise {
    pub seed: u32,
    pub temperature: OpenSimplex,
    pub humidity: OpenSimplex,
    pub continentalness: Fbm<OpenSimplex>,
    pub erosion: OpenSimplex,
    pub weirdness: Fbm<OpenSimplex>,
    pub depth: OpenSimplex,
    pub jagged: RidgedMulti<Perlin>, // sharp ridges for peaks
    pub surface: Perlin,             // high-freq surface detail
    pub offset: Perlin,              // micro surface noise
    pub river: RidgedMulti<Perlin>,  // low-freq ridged -> rivers where ~0
}

impl WorldNoise {
    pub fn new(seed: u32) -> Self {
        let s = |salt: u32| seed.wrapping_add(salt);
        Self {
            seed,
            temperature: OpenSimplex::new(s(0x111)),
            humidity: OpenSimplex::new(s(0x222)),
            // Continentalness: smooth large-scale landmass shape.
            // 3-octave fbm, period ~768 blocks (freq ~0.0013).
            continentalness: Fbm::<OpenSimplex>::new(s(0x333))
                .set_octaves(3)
                .set_frequency(0.0013),
            // Erosion: very-low-frequency overall terrain smoothness.
            erosion: OpenSimplex::new(s(0x444)),
            // Weirdness: medium-frequency rolling variation for hills/valleys.
            weirdness: Fbm::<OpenSimplex>::new(s(0x555))
                .set_octaves(4)
                .set_frequency(0.0055),
            depth: OpenSimplex::new(s(0x666)),
            // Jagged ridges: high-freq sharp peaks. Period ~80 blocks.
            jagged: RidgedMulti::<Perlin>::default()
                .set_seed(s(0x777)).set_octaves(3).set_frequency(0.012),
            surface: Perlin::new(s(0x888)),
            offset: Perlin::new(s(0x999)),
            // River channels: low-freq ridged lines. Period ~1500 blocks.
            river: RidgedMulti::<Perlin>::default()
                .set_seed(s(0xAAA)).set_octaves(2).set_frequency(0.000_65),
        }
    }

    /// Climate sample at world (x,z) — produces 6-parameter tuple.
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
    /// Combines continent/erosion/weirdness base height with jagged peaks
    /// and small offset/surface detail. Returns absolute world Y.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let fx = x as f64;
        let fz = z as f64;

        // Continentalness (fbm, ~[-1,1]): drives major land vs ocean shape.
        let cont = self.continentalness.get([fx, fz]);
        // Map continent to a base height. Sea level is 64; we want:
        //   cont < -0.15  -> deep ocean (~38..58)
        //   cont ≈ 0      -> coast / beaches (~62..70)
        //   cont > 0.3    -> inland plains / hills (~72..95)
        //   cont > 0.7    -> mountainous base (~95..120)
        // Using a nonlinear ramp so most terrain sits comfortably above sea.
        let cont01 = (cont * 0.5 + 0.5).clamp(0.0, 1.0);
        // base = 40 + 60 * smoothstep(0,1,cont01) gives [40..100].
        let base = 40.0 + 60.0 * smoothstep(0.0, 1.0, cont01 as f32) as f64;

        // Erosion: negative = rugged, positive = smooth/flat.
        let erosion = self.erosion.get([fx * 0.000_45, fz * 0.000_45]);
        let er_factor = (erosion * 0.5 + 0.5).clamp(0.0, 1.0); // 0 rough, 1 smooth

        // Weirdness (fbm, ~[-1,1]): medium-frequency hills and valleys.
        let weird = self.weirdness.get([fx, fz]);
        // Hills stronger where continent is high (inland) and erosion low.
        let hill_amp = (1.0 - 0.5 * er_factor) * (10.0 + 18.0 * cont01);
        let h = base + weird * hill_amp;

        // Jagged ridged noise for sharp peaks. Only meaningful inland + rugged.
        let jag = self.jagged.get([fx * 0.012, fz * 0.012]); // ~[0,1]
        let jag_amp = (1.0 - 0.85 * er_factor) * (8.0 + 22.0 * cont01);
        // Peaks gated so jagged only contributes on already-elevated terrain.
        let peak_gate = smoothstep(0.55, 0.95, jag as f32);
        let h = h + jag_amp * peak_gate as f64;

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
        // We carve where the value is near 0 (between ridges), gated by an
        // additional low-freq mask so rivers don't cover the whole world.
        let mask = self.depth.get([fx * 0.000_5, fz * 0.000_5]) as f32 * 0.5 + 0.5;
        let in_channel = (1.0 - (r as f32 - 0.0).abs().min(1.0)).powi(2);
        let strength = (in_channel - 0.85).max(0.0) / 0.15; // sharp band
        strength * smoothstep(0.4, 0.9, mask)
    }
}
