//! Seed-derived surface-terrain noise fields.
//!
//! This module intentionally does not use the cave-oriented `worldgen::noise`
//! samplers. Every field is a pure function of `(world_seed, field id, point)`.

use super::super::graph::{SamplePoint, SampledScalarField};

/// The 16 axis-aligned edge gradients (each pointing to an edge midpoint of the
/// unit cube, magnitude √2). This is the standard improved-Perlin gradient set:
/// the 12 distinct cube-edge directions plus 4 repeats, indexed by `hash & 15`.
///
/// The magnitude MUST be √2, not 1: the reference double-Perlin value factor
/// (`AMP_INI`) is calibrated for these √2 edge gradients. Normalizing them to
/// unit length silently shrinks every field by a factor of √2, breaking the
/// bit-exact parity and collapsing the climate axes toward zero (which starves
/// the outer classification bands — deep ocean, extremes).
const GRADIENTS: [(f64, f64, f64); 16] = [
    (1.0, 1.0, 0.0),
    (-1.0, 1.0, 0.0),
    (1.0, -1.0, 0.0),
    (-1.0, -1.0, 0.0),
    (1.0, 0.0, 1.0),
    (-1.0, 0.0, 1.0),
    (1.0, 0.0, -1.0),
    (-1.0, 0.0, -1.0),
    (0.0, 1.0, 1.0),
    (0.0, -1.0, 1.0),
    (0.0, 1.0, -1.0),
    (0.0, -1.0, -1.0),
    (1.0, 1.0, 0.0),
    (0.0, -1.0, 1.0),
    (-1.0, 1.0, 0.0),
    (0.0, -1.0, -1.0),
];

// === Exact reference noise (Xoroshiro128++) ===========================
//
// The reference generator seeds every Perlin octave from a Xoroshiro128++ stream,
// not our SplitMix/FNV scheme. To make `(seed, field, x, y, z)` reproduce the
// reference value-for-value, the permutation, origin offsets, and octave seeds
// must all come from this exact RNG. The sampling math (gradient set, fade, lerp,
// value factor) already matches; only the seeding differs.

#[derive(Clone, Copy, Debug)]
pub(crate) struct Xoroshiro {
    lo: u64,
    hi: u64,
}

impl Xoroshiro {
    pub(crate) fn new(value: u64) -> Self {
        const XL: u64 = 0x9e37_79b9_7f4a_7c15;
        const XH: u64 = 0x6a09_e667_f3bc_c909;
        const A: u64 = 0xbf58_476d_1ce4_e5b9;
        const B: u64 = 0x94d0_49bb_1331_11eb;
        let mut l = value ^ XH;
        let mut h = l.wrapping_add(XL);
        l = (l ^ (l >> 30)).wrapping_mul(A);
        h = (h ^ (h >> 30)).wrapping_mul(A);
        l = (l ^ (l >> 27)).wrapping_mul(B);
        h = (h ^ (h >> 27)).wrapping_mul(B);
        l ^= l >> 31;
        h ^= h >> 31;
        Self { lo: l, hi: h }
    }

    pub(crate) fn from_parts(lo: u64, hi: u64) -> Self {
        Self { lo, hi }
    }

    pub(crate) fn next_long(&mut self) -> u64 {
        let l = self.lo;
        let h = self.hi;
        let n = rotl64(l.wrapping_add(h), 17).wrapping_add(l);
        let h = h ^ l;
        self.lo = rotl64(l, 49) ^ h ^ (h << 21);
        self.hi = rotl64(h, 28);
        n
    }

    /// Bounded integer in `[0, n)`, matching the reference's Lemire-style draw
    /// (including the rejection loop, so the RNG stream advances identically).
    pub(crate) fn next_int(&mut self, n: u32) -> u32 {
        let mut r = (self.next_long() & 0xFFFF_FFFF).wrapping_mul(u64::from(n));
        if (r as u32) < n {
            let threshold = n.wrapping_neg() % n;
            while (r as u32) < threshold {
                r = (self.next_long() & 0xFFFF_FFFF).wrapping_mul(u64::from(n));
            }
        }
        (r >> 32) as u32
    }

    pub(crate) fn next_double(&mut self) -> f64 {
        (self.next_long() >> 11) as f64 * 1.110_223_024_625_156_5e-16
    }
}

fn rotl64(x: u64, b: u32) -> u64 {
    x.rotate_left(b)
}

/// md5 of "octave_-12".."octave_0"; index `12 + omin + i` selects octave `i`'s seed.
const MD5_OCTAVE: [(u64, u64); 13] = [
    (0xb198_de63_a801_2672, 0x7b84_cad4_3ef7_b5a8),
    (0x0fd7_87bf_bc40_3ec3, 0x74a4_a31c_a21b_48b8),
    (0x36d3_26ee_d40e_feb2, 0x5be9_ce18_223c_636a),
    (0x082f_e255_f8be_6631, 0x4e96_119e_22de_dc81),
    (0x0ef6_8ec6_8504_005e, 0x48b6_bf93_a278_9640),
    (0xf112_6812_8982_754f, 0x257a_1d67_0430_b0aa),
    (0xe51c_98ce_7d1d_e664, 0x5f94_78a7_3304_0c45),
    (0x6d7b_49e7_e429_850a, 0x2e30_63c6_22a2_4777),
    (0xbd90_d537_7ba1_b762, 0xc073_17d4_19a7_548d),
    (0x53d3_9c67_52da_c858, 0xbcd1_c5a8_0ab6_5b3e),
    (0xb4a2_4d7a_84e7_677b, 0x023f_f966_8e89_b5c4),
    (0xdffa_22b5_34c5_f608, 0xb9b6_7517_d366_5ca9),
    (0xd507_0808_6cef_4d7c, 0x6e16_51ec_c7f4_3309),
];

/// Lowest-octave frequency by `-omin` (= `2^omin`); doubled per octave.
const LACUNA_INI: [f64; 13] = [
    1.0,
    0.5,
    0.25,
    1.0 / 8.0,
    1.0 / 16.0,
    1.0 / 32.0,
    1.0 / 64.0,
    1.0 / 128.0,
    1.0 / 256.0,
    1.0 / 512.0,
    1.0 / 1024.0,
    1.0 / 2048.0,
    1.0 / 4096.0,
];

/// Lowest-octave amplitude weight by octave count `len`; halved per octave.
const PERSIST_INI: [f64; 10] = [
    0.0,
    1.0,
    2.0 / 3.0,
    4.0 / 7.0,
    8.0 / 15.0,
    16.0 / 31.0,
    32.0 / 63.0,
    64.0 / 127.0,
    128.0 / 255.0,
    256.0 / 511.0,
];

/// Double-Perlin value factor by trimmed octave count `len` (= `(5/3)·len/(len+1)`).
const AMP_INI: [f64; 10] = [
    0.0,
    5.0 / 6.0,
    10.0 / 9.0,
    15.0 / 12.0,
    20.0 / 15.0,
    25.0 / 18.0,
    30.0 / 21.0,
    35.0 / 24.0,
    40.0 / 27.0,
    45.0 / 30.0,
];

/// One Perlin octave: a permutation, a sampling origin `(a, b, c)`, and the
/// amplitude/lacunarity it contributes to its octave stack. Built by the exact
/// reference `xPerlinInit`: origins from `next_double·256`, then a Fisher-Yates
/// shuffle driven by `next_int`.
#[derive(Clone, Debug)]
struct PerlinOctave {
    perm: [u8; 257],
    a: f64,
    b: f64,
    c: f64,
    amplitude: f64,
    lacunarity: f64,
    /// The b-axis (Y) lattice cell / fractional / fade precomputed for `y == 0` — every
    /// climate and surface-density sample passes `y = 0` (all live noise is 2D), so the
    /// reference `d2 == 0` fast path is hit on every call. Holds exactly what
    /// `sample` would compute from `y + b` at `y = 0`, so results stay bit-identical.
    h2_y0: u8,
    d2_y0: f64,
    t2_y0: f64,
}

impl PerlinOctave {
    fn init(xr: &mut Xoroshiro) -> Self {
        let a = xr.next_double() * 256.0;
        let b = xr.next_double() * 256.0;
        let c = xr.next_double() * 256.0;
        let mut perm = [0u8; 257];
        for (i, slot) in perm.iter_mut().take(256).enumerate() {
            *slot = i as u8;
        }
        for i in 0..256u32 {
            let j = xr.next_int(256 - i) + i;
            perm.swap(i as usize, j as usize);
        }
        perm[256] = perm[0];
        let i2 = b.floor();
        let d2_y0 = b - i2;
        Self {
            perm,
            a,
            b,
            c,
            amplitude: 1.0,
            lacunarity: 1.0,
            h2_y0: i2 as i64 as u8,
            d2_y0,
            t2_y0: fade(d2_y0),
        }
    }

    /// `samplePerlin` with `yamp = ymin = 0` (the climate/octave call). The `d2==0`
    /// fast path in the reference is a pure optimization — this general path yields
    /// identical values, so it is omitted.
    fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut d1 = x + self.a;
        let mut d3 = z + self.c;
        let i1 = d1.floor();
        let i3 = d3.floor();
        d1 -= i1;
        d3 -= i3;
        let h1 = i1 as i64 as u8;
        let h3 = i3 as i64 as u8;
        let t1 = fade(d1);
        let t3 = fade(d3);
        // The reference `d2 == 0` fast path: at y == 0 the Y-axis cell/frac/fade are the
        // precomputed b-axis constants (identical to floor/fade of `y + b`), saving a
        // floor + a fade polynomial on every octave sample. Any non-zero y (none today)
        // falls back to the exact general computation.
        let (d2, h2, t2) = if y == 0.0 {
            (self.d2_y0, self.h2_y0, self.t2_y0)
        } else {
            let raw = y + self.b;
            let i2 = raw.floor();
            let frac = raw - i2;
            (frac, i2 as i64 as u8, fade(frac))
        };
        let idx = &self.perm;
        let a1 = idx[h1 as usize].wrapping_add(h2);
        let b1 = idx[h1 as usize + 1].wrapping_add(h2);
        let a2 = idx[a1 as usize].wrapping_add(h3);
        let b2 = idx[b1 as usize].wrapping_add(h3);
        let a3 = idx[a1 as usize + 1].wrapping_add(h3);
        let b3 = idx[b1 as usize + 1].wrapping_add(h3);
        let l1 = grad_dot(idx[a2 as usize], d1, d2, d3);
        let l2 = grad_dot(idx[b2 as usize], d1 - 1.0, d2, d3);
        let l3 = grad_dot(idx[a3 as usize], d1, d2 - 1.0, d3);
        let l4 = grad_dot(idx[b3 as usize], d1 - 1.0, d2 - 1.0, d3);
        let l5 = grad_dot(idx[a2 as usize + 1], d1, d2, d3 - 1.0);
        let l6 = grad_dot(idx[b2 as usize + 1], d1 - 1.0, d2, d3 - 1.0);
        let l7 = grad_dot(idx[a3 as usize + 1], d1, d2 - 1.0, d3 - 1.0);
        let l8 = grad_dot(idx[b3 as usize + 1], d1 - 1.0, d2 - 1.0, d3 - 1.0);
        let l1 = rlerp(t1, l1, l2);
        let l3 = rlerp(t1, l3, l4);
        let l5 = rlerp(t1, l5, l6);
        let l7 = rlerp(t1, l7, l8);
        let l1 = rlerp(t2, l1, l3);
        let l5 = rlerp(t2, l5, l7);
        rlerp(t3, l1, l5)
    }
}

/// A stack of Perlin octaves (`xOctaveInit` / `sampleOctave`). `nmax < 0` keeps all
/// octaves (the climate-field case).
#[derive(Clone, Debug)]
struct OctaveStack {
    octaves: Vec<PerlinOctave>,
}

impl OctaveStack {
    fn init(xr: &mut Xoroshiro, amplitudes: &[f64], omin: i32, nmax: i32) -> Self {
        let len = amplitudes.len();
        let mut lacuna = LACUNA_INI[(-omin) as usize];
        let mut persist = PERSIST_INI[len];
        let xlo = xr.next_long();
        let xhi = xr.next_long();
        let mut octaves = Vec::new();
        let mut n = 0i32;
        for (i, &amp) in amplitudes.iter().enumerate() {
            if nmax >= 0 && n == nmax {
                break;
            }
            if amp != 0.0 {
                let salt = MD5_OCTAVE[(12 + omin + i as i32) as usize];
                let mut pxr = Xoroshiro::from_parts(xlo ^ salt.0, xhi ^ salt.1);
                let mut octave = PerlinOctave::init(&mut pxr);
                octave.amplitude = amp * persist;
                octave.lacunarity = lacuna;
                octaves.push(octave);
                n += 1;
            }
            lacuna *= 2.0;
            persist *= 0.5;
        }
        Self { octaves }
    }

    fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut v = 0.0;
        for octave in &self.octaves {
            let lf = octave.lacunarity;
            v += octave.amplitude * octave.sample(x * lf, y * lf, z * lf);
        }
        v
    }
}

/// Exact reference double-Perlin: two independent octave stacks summed (the second
/// at input ×337/331) and scaled by the trimmed-octave value factor. Built by
/// `xDoublePerlinInit`; the climate fields pass `nmax = -1` (both stacks full).
#[derive(Clone, Debug)]
pub(crate) struct ReferenceDoublePerlin {
    oct_a: OctaveStack,
    oct_b: OctaveStack,
    amplitude: f64,
}

impl ReferenceDoublePerlin {
    fn init(xr: &mut Xoroshiro, amplitudes: &[f64], omin: i32) -> Self {
        let oct_a = OctaveStack::init(xr, amplitudes, omin, -1);
        let oct_b = OctaveStack::init(xr, amplitudes, omin, -1);
        let first = amplitudes.iter().position(|&a| a != 0.0);
        let last = amplitudes.iter().rposition(|&a| a != 0.0);
        let eff_len = match (first, last) {
            (Some(f), Some(l)) => l - f + 1,
            _ => 0,
        };
        Self {
            oct_a,
            oct_b,
            amplitude: AMP_INI[eff_len],
        }
    }

    pub(crate) fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        const F: f64 = 337.0 / 331.0;
        (self.oct_a.sample(x, y, z) + self.oct_b.sample(x * F, y * F, z * F)) * self.amplitude
    }
}

/// A climate field's seed fork: its md5 salt (XORed into the world-seed Xoroshiro),
/// first octave, and octave amplitudes.
pub(crate) struct ClimateFieldParams {
    pub salt: (u64, u64),
    pub omin: i32,
    pub amplitudes: &'static [f64],
}

/// The reference climate fields. All fork from the same `xSetSeed(world_seed)`
/// stream (its first two longs), then XOR their per-field md5 salt.
pub(crate) mod climate_fields {
    use super::ClimateFieldParams;

    pub(crate) const TEMPERATURE: ClimateFieldParams = ClimateFieldParams {
        salt: (0x5c7e_6b29_735f_0d7f, 0xf7d8_6f1b_bc73_4988),
        omin: -10,
        amplitudes: &[1.5, 0.0, 1.0, 0.0, 0.0, 0.0],
    };
    pub(crate) const HUMIDITY: ClimateFieldParams = ClimateFieldParams {
        salt: (0x81bb_4d22_e8dc_168e, 0xf1c8_b4be_a163_03cd),
        omin: -8,
        amplitudes: &[1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
    };
    pub(crate) const CONTINENTALITY: ClimateFieldParams = ClimateFieldParams {
        salt: (0x8388_6c9d_0ae3_a662, 0xafa6_38a6_1b42_e8ad),
        omin: -9,
        amplitudes: &[1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0],
    };
    pub(crate) const EROSION: ClimateFieldParams = ClimateFieldParams {
        salt: (0xd024_91e6_058f_6fd8, 0x4792_512c_94c1_7a80),
        omin: -9,
        amplitudes: &[1.0, 1.0, 0.0, 1.0, 1.0],
    };
    pub(crate) const SHIFT: ClimateFieldParams = ClimateFieldParams {
        salt: (0x0805_18cf_6af2_5384, 0x3f3d_fb40_a54f_ebd5),
        omin: -3,
        amplitudes: &[1.0, 1.0, 1.0, 0.0],
    };
    pub(crate) const WEIRDNESS: ClimateFieldParams = ClimateFieldParams {
        salt: (0xefc8_ef4d_3610_2b34, 0x1bee_eb32_4a0f_24ea),
        omin: -7,
        amplitudes: &[1.0, 2.0, 1.0, 0.0, 0.0, 0.0],
    };
}

/// Build a climate field's double-Perlin noise for a world seed, exactly as the
/// reference `setBiomeSeed`/`init_climate_seed` does.
pub(crate) fn build_climate_field(
    world_seed: u64,
    params: &ClimateFieldParams,
) -> ReferenceDoublePerlin {
    let mut xr = Xoroshiro::new(world_seed);
    let xlo = xr.next_long();
    let xhi = xr.next_long();
    let mut pxr = Xoroshiro::from_parts(xlo ^ params.salt.0, xhi ^ params.salt.1);
    ReferenceDoublePerlin::init(&mut pxr, params.amplitudes, params.omin)
}

/// A climate axis sampled with the reference domain warp: the shift field warps
/// the sample coordinates before the axis field is read. Horizontal (y-invariant),
/// sampled at the 1:4 quart scale (world block → quart cell).
#[derive(Clone, Debug)]
pub(crate) struct ShiftedClimateField {
    shift: ReferenceDoublePerlin,
    field: ReferenceDoublePerlin,
}

impl ShiftedClimateField {
    pub(crate) fn new(world_seed: u64, params: &ClimateFieldParams) -> Self {
        Self {
            shift: build_climate_field(world_seed, &climate_fields::SHIFT),
            field: build_climate_field(world_seed, params),
        }
    }

    /// Sample at quart coordinates (the reference's native climate scale). The shift
    /// for `z` reads the shift field at `(qz, qx, 0)` — swapped, matching the source.
    pub(crate) fn sample_quart(&self, qx: f64, qz: f64) -> f64 {
        let sx = self.shift.sample(qx, 0.0, qz) * 4.0;
        let sz = self.shift.sample(qz, qx, 0.0) * 4.0;
        self.field.sample(qx + sx, 0.0, qz + sz)
    }
}

impl SampledScalarField for ShiftedClimateField {
    fn sample(&self, point: SamplePoint) -> f64 {
        self.sample_quart(point.x * 0.25, point.z * 0.25)
    }

    fn depends_on_y(&self) -> bool {
        false
    }
}

fn grad_dot(hash: u8, dx: f64, dy: f64, dz: f64) -> f64 {
    let (gx, gy, gz) = GRADIENTS[(hash & 0xf) as usize];
    gx * dx + gy * dy + gz * dz
}

/// Reference-order lerp (`from + part·(to − from)`), matching cubiomes' `lerp`.
fn rlerp(part: f64, from: f64, to: f64) -> f64 {
    from + part * (to - from)
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perlin_gradients_have_root_two_magnitude() {
        // The reference double-Perlin value factor (`AMP_INI`) is calibrated for
        // edge gradients of magnitude √2. Normalizing them to unit length silently
        // shrinks every field by √2, breaking parity and compressing the climate
        // axes. This invariant has regressed before; lock the magnitude.
        for &(x, y, z) in &GRADIENTS {
            let mag_sq = x * x + y * y + z * z;
            assert!(
                (mag_sq - 2.0).abs() < 1.0e-12,
                "gradient {:?} must have magnitude √2 (squared = 2), got {mag_sq}",
                (x, y, z)
            );
        }
    }
}
