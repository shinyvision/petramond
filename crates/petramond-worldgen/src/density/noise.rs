use super::super::graph::{SamplePoint, SampledScalarField};
use crate::cache::local::{self, LocalTable};

#[cfg(target_arch = "x86_64")]
mod avx2;
mod lanes;
use lanes::{gradient_pair, Lanes, F2};

#[derive(Clone, Copy, Debug)]
pub struct Xoroshiro {
    lo: u64,
    hi: u64,
}

impl Xoroshiro {
    pub fn new(value: u64) -> Self {
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

    pub fn from_parts(lo: u64, hi: u64) -> Self {
        Self { lo, hi }
    }

    pub fn next_long(&mut self) -> u64 {
        let l = self.lo;
        let h = self.hi;
        let n = rotl64(l.wrapping_add(h), 17).wrapping_add(l);
        let h = h ^ l;
        self.lo = rotl64(l, 49) ^ h ^ (h << 21);
        self.hi = rotl64(h, 28);
        n
    }

    pub fn next_int(&mut self, n: u32) -> u32 {
        let mut r = (self.next_long() & 0xFFFF_FFFF).wrapping_mul(u64::from(n));
        if (r as u32) < n {
            let threshold = n.wrapping_neg() % n;
            while (r as u32) < threshold {
                r = (self.next_long() & 0xFFFF_FFFF).wrapping_mul(u64::from(n));
            }
        }
        (r >> 32) as u32
    }

    pub fn next_double(&mut self) -> f64 {
        (self.next_long() >> 11) as f64 * 1.110_223_024_625_156_5e-16
    }
}

fn rotl64(x: u64, b: u32) -> u64 {
    x.rotate_left(b)
}

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

#[derive(Clone, Debug)]
struct PerlinOctave {
    perm: [u8; 257],
    origin: [f64; 3],
    amplitude: f64,
    lacunarity: f64,
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
        Self {
            perm,
            origin: [a, b, c],
            amplitude: 1.0,
            lacunarity: 1.0,
        }
    }
}

fn octave_stack(xr: &mut Xoroshiro, amplitudes: &[f64], omin: i32) -> Vec<PerlinOctave> {
    let len = amplitudes.len();
    let mut lacuna = LACUNA_INI[(-omin) as usize];
    let mut persist = PERSIST_INI[len];
    let xlo = xr.next_long();
    let xhi = xr.next_long();
    let mut octaves = Vec::new();
    for (i, &amp) in amplitudes.iter().enumerate() {
        if amp != 0.0 {
            let salt = MD5_OCTAVE[(12 + omin + i as i32) as usize];
            let mut pxr = Xoroshiro::from_parts(xlo ^ salt.0, xhi ^ salt.1);
            let mut octave = PerlinOctave::init(&mut pxr);
            octave.amplitude = amp * persist;
            octave.lacunarity = lacuna;
            octaves.push(octave);
        }
        lacuna *= 2.0;
        persist *= 0.5;
    }
    octaves
}

/// Octave `i` of both stacks. The stacks share their amplitude and
/// lacunarity sequences, so one lane evaluates each.
#[derive(Clone, Debug)]
struct OctavePair {
    perm: [[u8; 257]; 2],
    origin: [[f64; 2]; 3],
    amplitude: f64,
    lacunarity: f64,
}

/// A point's position inside its lattice cell, per lane.
struct Cell<L> {
    frac: [L; 3],
    hash: [[u8; 2]; 3],
}

impl OctavePair {
    #[inline(always)]
    fn cell<L: Lanes>(&self, x: L, y: L, z: L) -> Cell<L> {
        let o = &self.origin;
        let (i1, h1) = (x + L::new(o[0][0], o[0][1])).floor_hash();
        let (i2, h2) = (y + L::new(o[1][0], o[1][1])).floor_hash();
        let (i3, h3) = (z + L::new(o[2][0], o[2][1])).floor_hash();
        Cell {
            frac: [
                x + L::new(o[0][0], o[0][1]) - i1,
                y + L::new(o[1][0], o[1][1]) - i2,
                z + L::new(o[2][0], o[2][1]) - i3,
            ],
            hash: [h1, h2, h3],
        }
    }

    /// The gradient pairs (see [`gradient_pair`]) of a cell's eight corners.
    #[inline(always)]
    fn corners(&self, [h1, h2, h3]: [[u8; 2]; 3]) -> [u8; 8] {
        let hash = |lane: usize| {
            let idx = &self.perm[lane];
            let (h1, h2, h3) = (h1[lane], h2[lane], h3[lane]);
            let a1 = idx[h1 as usize].wrapping_add(h2);
            let b1 = idx[h1 as usize + 1].wrapping_add(h2);
            let a2 = idx[a1 as usize].wrapping_add(h3) as usize;
            let b2 = idx[b1 as usize].wrapping_add(h3) as usize;
            let a3 = idx[a1 as usize + 1].wrapping_add(h3) as usize;
            let b3 = idx[b1 as usize + 1].wrapping_add(h3) as usize;
            [
                idx[a2],
                idx[b2],
                idx[a3],
                idx[b3],
                idx[a2 + 1],
                idx[b2 + 1],
                idx[a3 + 1],
                idx[b3 + 1],
            ]
        };
        let (p, q) = (hash(0), hash(1));
        std::array::from_fn(|k| gradient_pair(p[k], q[k]))
    }

    #[inline(always)]
    fn blend<L: Lanes>(g: &[u8; 8], [d1, d2, d3]: [L; 3]) -> L {
        let (t1, t2, t3) = (d1.fade(), d2.fade(), d3.fade());
        let one = L::splat(1.0);
        let (e1, e2, e3) = (d1 - one, d2 - one, d3 - one);
        let l1 = L::grad(g[0], d1, d2, d3);
        let l2 = L::grad(g[1], e1, d2, d3);
        let l3 = L::grad(g[2], d1, e2, d3);
        let l4 = L::grad(g[3], e1, e2, d3);
        let l5 = L::grad(g[4], d1, d2, e3);
        let l6 = L::grad(g[5], e1, d2, e3);
        let l7 = L::grad(g[6], d1, e2, e3);
        let l8 = L::grad(g[7], e1, e2, e3);
        let l1 = L::lerp(t1, l1, l2);
        let l3 = L::lerp(t1, l3, l4);
        let l5 = L::lerp(t1, l5, l6);
        let l7 = L::lerp(t1, l7, l8);
        let l1 = L::lerp(t2, l1, l3);
        let l5 = L::lerp(t2, l5, l7);
        L::lerp(t3, l1, l5)
    }

    #[inline(always)]
    fn sample<L: Lanes>(&self, x: L, y: L, z: L) -> L {
        let cell = self.cell(x, y, z);
        Self::blend(&self.corners(cell.hash), cell.frac)
    }
}

#[derive(Clone, Debug)]
pub struct ReferenceDoublePerlin {
    octaves: Box<[OctavePair]>,
    amplitude: f64,
}

impl ReferenceDoublePerlin {
    pub(crate) fn from_seed(
        world_seed: u64,
        salt: [u64; 2],
        first_octave: i32,
        amplitudes: &[f64],
    ) -> Self {
        let mut root = Xoroshiro::new(world_seed);
        let mut fork =
            Xoroshiro::from_parts(root.next_long() ^ salt[0], root.next_long() ^ salt[1]);
        Self::init(&mut fork, amplitudes, first_octave)
    }

    fn init(xr: &mut Xoroshiro, amplitudes: &[f64], omin: i32) -> Self {
        let oct_a = octave_stack(xr, amplitudes, omin);
        let oct_b = octave_stack(xr, amplitudes, omin);
        let first = amplitudes.iter().position(|&a| a != 0.0);
        let last = amplitudes.iter().rposition(|&a| a != 0.0);
        let eff_len = match (first, last) {
            (Some(f), Some(l)) => l - f + 1,
            _ => 0,
        };
        let octaves = oct_a
            .into_iter()
            .zip(oct_b)
            .map(|(a, b)| OctavePair {
                perm: [a.perm, b.perm],
                origin: std::array::from_fn(|axis| [a.origin[axis], b.origin[axis]]),
                amplitude: a.amplitude,
                lacunarity: a.lacunarity,
            })
            .collect();
        Self {
            octaves,
            amplitude: AMP_INI[eff_len],
        }
    }

    pub fn sample(&self, x: f64, y: f64, z: f64) -> f64 {
        self.sample_lanes::<F2>(x, y, z)
    }

    /// `out[i] = sample(x[i], y[i], z[i])`, bit for bit. Octaves run outermost
    /// so each octave's tables stay hot, and a point in the same cell as the
    /// one before it reuses that cell's corner hashes.
    pub fn sample_many(&self, x: &[f64], y: &[f64], z: &[f64], out: &mut [f64]) {
        #[cfg(target_arch = "x86_64")]
        let wide = std::arch::is_x86_feature_detected!("avx2");
        #[cfg(not(target_arch = "x86_64"))]
        let wide = false;
        self.sample_many_with(wide, x, y, z, out);
    }

    /// [`Self::sample_many`] with the four-lane kernel chosen by the caller;
    /// `wide` is honoured only on x86-64 with AVX2 present.
    fn sample_many_with(&self, wide: bool, x: &[f64], y: &[f64], z: &[f64], out: &mut [f64]) {
        const CHUNK: usize = 64;
        #[cfg(target_arch = "x86_64")]
        let wide = wide && std::arch::is_x86_feature_detected!("avx2");
        for start in (0..out.len()).step_by(CHUNK) {
            let end = (start + CHUNK).min(out.len());
            let (x, y, z) = (&x[start..end], &y[start..end], &z[start..end]);
            let out = &mut out[start..end];
            #[cfg(target_arch = "x86_64")]
            if wide {
                // SAFETY: AVX2 support was checked above.
                unsafe { avx2::sample_chunk(self, x, y, z, out) };
                continue;
            }
            let _ = wide;
            self.sample_chunk::<F2>(x, y, z, out);
        }
    }

    #[inline(always)]
    fn sample_chunk<L: Lanes>(&self, x: &[f64], y: &[f64], z: &[f64], out: &mut [f64]) {
        const F: f64 = 337.0 / 331.0;
        let n = out.len();
        let zero = L::splat(0.0);
        let mut p = [[zero; 3]; 64];
        let mut acc = [zero; 64];
        for i in 0..n {
            p[i] = [
                L::new(x[i], x[i] * F),
                L::new(y[i], y[i] * F),
                L::new(z[i], z[i] * F),
            ];
        }
        for octave in self.octaves.iter() {
            let lf = L::splat(octave.lacunarity);
            let amp = L::splat(octave.amplitude);
            let mut last = None;
            let mut g = [0u8; 8];
            for i in 0..n {
                let [px, py, pz] = p[i];
                let cell = octave.cell(px * lf, py * lf, pz * lf);
                if last != Some(cell.hash) {
                    g = octave.corners(cell.hash);
                    last = Some(cell.hash);
                }
                acc[i] = acc[i] + amp * OctavePair::blend(&g, cell.frac);
            }
        }
        for i in 0..n {
            let [a, b] = acc[i].to_array();
            out[i] = (a + b) * self.amplitude;
        }
    }

    #[inline(always)]
    fn sample_lanes<L: Lanes>(&self, x: f64, y: f64, z: f64) -> f64 {
        const F: f64 = 337.0 / 331.0;
        let (x, y, z) = (L::new(x, x * F), L::new(y, y * F), L::new(z, z * F));
        let mut v = L::splat(0.0);
        for octave in self.octaves.iter() {
            let lf = L::splat(octave.lacunarity);
            v = v + L::splat(octave.amplitude) * octave.sample(x * lf, y * lf, z * lf);
        }
        let [a, b] = v.to_array();
        (a + b) * self.amplitude
    }
}

pub struct ClimateFieldParams {
    pub salt: (u64, u64),
    pub omin: i32,
    pub amplitudes: &'static [f64],
}

/// All of these fork off the first two longs of `xSetSeed(world_seed)`, then XOR in their own md5
/// salt.
pub mod climate_fields {
    use super::ClimateFieldParams;

    pub const TEMPERATURE: ClimateFieldParams = ClimateFieldParams {
        salt: (0x5c7e_6b29_735f_0d7f, 0xf7d8_6f1b_bc73_4988),
        omin: -10,
        amplitudes: &[1.5, 0.0, 1.0, 0.0, 0.0, 0.0],
    };
    pub const HUMIDITY: ClimateFieldParams = ClimateFieldParams {
        salt: (0x81bb_4d22_e8dc_168e, 0xf1c8_b4be_a163_03cd),
        omin: -8,
        amplitudes: &[1.0, 1.0, 0.0, 0.0, 0.0, 0.0],
    };
    pub const CONTINENTALITY: ClimateFieldParams = ClimateFieldParams {
        salt: (0x8388_6c9d_0ae3_a662, 0xafa6_38a6_1b42_e8ad),
        omin: -9,
        amplitudes: &[1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0],
    };
    pub const EROSION: ClimateFieldParams = ClimateFieldParams {
        salt: (0xd024_91e6_058f_6fd8, 0x4792_512c_94c1_7a80),
        omin: -9,
        amplitudes: &[1.0, 1.0, 0.0, 1.0, 1.0],
    };
    pub const SHIFT: ClimateFieldParams = ClimateFieldParams {
        salt: (0x0805_18cf_6af2_5384, 0x3f3d_fb40_a54f_ebd5),
        omin: -3,
        amplitudes: &[1.0, 1.0, 1.0, 0.0],
    };
    pub const WEIRDNESS: ClimateFieldParams = ClimateFieldParams {
        salt: (0xefc8_ef4d_3610_2b34, 0x1bee_eb32_4a0f_24ea),
        omin: -7,
        amplitudes: &[1.0, 2.0, 1.0, 0.0, 0.0, 0.0],
    };
    pub const CRAG: ClimateFieldParams = ClimateFieldParams {
        salt: (0x11a3_c0de_5eed_2026, 0x0707_beef_cafe_f00d),
        omin: -5,
        amplitudes: &[1.0, 1.0],
    };
    pub const STRUCTURE: ClimateFieldParams = ClimateFieldParams {
        salt: (0x5a11_e175_0f00_ba12, 0x9e3d_77aa_1234_c001),
        omin: -10,
        amplitudes: &[1.0],
    };
}

pub fn build_climate_field(world_seed: u64, params: &ClimateFieldParams) -> ReferenceDoublePerlin {
    let mut xr = Xoroshiro::new(world_seed);
    let xlo = xr.next_long();
    let xhi = xr.next_long();
    let mut pxr = Xoroshiro::from_parts(xlo ^ params.salt.0, xhi ^ params.salt.1);
    ReferenceDoublePerlin::init(&mut pxr, params.amplitudes, params.omin)
}

#[derive(Clone, Debug)]
pub struct ShiftedClimateField {
    world_seed: u64,
    shift: ReferenceDoublePerlin,
    field: ReferenceDoublePerlin,
}

thread_local! {
    /// Per-thread memo of the climate domain warp, keyed by `(seed, qx, qz)`
    /// bits. Every [`ShiftedClimateField`] of a given world seed embeds the SAME
    /// shift field (`climate_fields::SHIFT` forks only from the world seed), so
    /// temperature/humidity/continentality/erosion/weirdness all recompute an
    /// identical `(sx, sz)` at the same quart cell — as do the height (density
    /// lattice) and biome (climate cell) passes, which sample the same
    /// world-anchored 4-block grid. Memoizing the warp is bit-exact: the cached
    /// values are the very f64s the direct computation yields.
    static WARP_MEMO: LocalTable<[u64; 3], (f64, f64)> =
        LocalTable::new(&local::CLIMATE_WARP);
}

impl ShiftedClimateField {
    pub fn new(world_seed: u64, params: &ClimateFieldParams) -> Self {
        Self {
            world_seed,
            shift: build_climate_field(world_seed, &climate_fields::SHIFT),
            field: build_climate_field(world_seed, params),
        }
    }

    pub fn sample_quart(&self, qx: f64, qz: f64) -> f64 {
        let (sx, sz) = self.warp(qx, qz);
        self.field.sample(qx + sx, 0.0, qz + sz)
    }

    fn warp(&self, qx: f64, qz: f64) -> (f64, f64) {
        let (qxb, qzb) = (qx.to_bits(), qz.to_bits());
        let hash = local::spread(qxb ^ qzb.rotate_left(32) ^ self.world_seed);
        WARP_MEMO.with(|memo| {
            memo.get_or_insert_with(hash, [self.world_seed, qxb, qzb], || {
                let sx = self.shift.sample(qx, 0.0, qz) * 4.0;
                let sz = self.shift.sample(qz, qx, 0.0) * 4.0;
                (sx, sz)
            })
        })
    }
}

impl SampledScalarField for ShiftedClimateField {
    fn sample(&self, point: SamplePoint) -> f64 {
        self.sample_quart(point.x * 0.25, point.z * 0.25)
    }

    fn sample_batch(&self, points: &[SamplePoint], out: &mut [f64]) {
        let mut xs = Vec::with_capacity(points.len());
        let mut zs = Vec::with_capacity(points.len());
        for point in points {
            let (qx, qz) = (point.x * 0.25, point.z * 0.25);
            let (sx, sz) = self.warp(qx, qz);
            xs.push(qx + sx);
            zs.push(qz + sz);
        }
        let ys = vec![0.0; points.len()];
        self.field.sample_many(&xs, &ys, &zs, out);
    }

    fn depends_on_y(&self) -> bool {
        false
    }
}

#[inline]
fn floor(x: f64) -> f64 {
    if x.is_nan() || x.abs() >= 4_503_599_627_370_496.0 {
        return x.floor();
    }
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t.copysign(x)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn floor_matches_libm_bit_for_bit() {
        let mut x = -1.0e6f64;
        while x < 1.0e6 {
            for v in [x, x.next_up(), x.next_down(), x.trunc(), -x.trunc()] {
                assert_eq!(super::floor(v).to_bits(), v.floor().to_bits(), "{v}");
            }
            x += 0.371;
        }
        for v in [
            0.0,
            -0.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            4.5e15,
            -4.5e15,
            1e300,
            -1e-300,
        ] {
            assert_eq!(super::floor(v).to_bits(), v.floor().to_bits(), "{v}");
        }
    }

    use super::*;

    #[test]
    fn warp_memo_is_bit_exact_across_interleaved_seeds_and_fields() {
        let fields: Vec<ShiftedClimateField> = vec![
            ShiftedClimateField::new(0x1234_5678, &climate_fields::CONTINENTALITY),
            ShiftedClimateField::new(0x1234_5678, &climate_fields::EROSION),
            ShiftedClimateField::new(0x1234_5678, &climate_fields::WEIRDNESS),
            ShiftedClimateField::new(0xDEAD_BEEF, &climate_fields::EROSION),
        ];

        let mut expected = std::collections::VecDeque::new();
        for pass in 0..2 {
            for q in 0..64 {
                let (qx, qz) = ((q % 8) as f64, (q / 8) as f64);
                for f in &fields {
                    let got = f.sample_quart(qx, qz);
                    if pass == 0 {
                        let sx = f.shift.sample(qx, 0.0, qz) * 4.0;
                        let sz = f.shift.sample(qz, qx, 0.0) * 4.0;
                        let inline = f.field.sample(qx + sx, 0.0, qz + sz);
                        assert_eq!(got.to_bits(), inline.to_bits());
                        expected.push_back(got);
                    } else {
                        let want = expected.pop_front().unwrap();
                        assert_eq!(got.to_bits(), want.to_bits());
                    }
                }
            }
        }
    }

    /// The scalar double-Perlin both lane implementations must reproduce.
    fn scalar_sample(noise: &ReferenceDoublePerlin, x: f64, y: f64, z: f64) -> f64 {
        let grads: Vec<_> = lanes::gradients().collect();
        let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
        let octave = |o: &OctavePair, lane: usize, x: f64, y: f64, z: f64| {
            let p = &o.perm[lane];
            let d = [
                x + o.origin[0][lane],
                y + o.origin[1][lane],
                z + o.origin[2][lane],
            ];
            let i = d.map(floor);
            let h = i.map(|v| v as i64 as u8);
            let (d1, d2, d3) = (d[0] - i[0], d[1] - i[1], d[2] - i[2]);
            let grad = |hash: u8, dx: f64, dy: f64, dz: f64| {
                let (gx, gy, gz) = grads[usize::from(hash & 15)];
                gx * dx + gy * dy + gz * dz
            };
            let a1 = p[h[0] as usize].wrapping_add(h[1]);
            let b1 = p[h[0] as usize + 1].wrapping_add(h[1]);
            let a2 = p[a1 as usize].wrapping_add(h[2]) as usize;
            let b2 = p[b1 as usize].wrapping_add(h[2]) as usize;
            let a3 = p[a1 as usize + 1].wrapping_add(h[2]) as usize;
            let b3 = p[b1 as usize + 1].wrapping_add(h[2]) as usize;
            let x0 = lerp(
                fade(d1),
                grad(p[a2], d1, d2, d3),
                grad(p[b2], d1 - 1.0, d2, d3),
            );
            let x1 = lerp(
                fade(d1),
                grad(p[a3], d1, d2 - 1.0, d3),
                grad(p[b3], d1 - 1.0, d2 - 1.0, d3),
            );
            let x2 = lerp(
                fade(d1),
                grad(p[a2 + 1], d1, d2, d3 - 1.0),
                grad(p[b2 + 1], d1 - 1.0, d2, d3 - 1.0),
            );
            let x3 = lerp(
                fade(d1),
                grad(p[a3 + 1], d1, d2 - 1.0, d3 - 1.0),
                grad(p[b3 + 1], d1 - 1.0, d2 - 1.0, d3 - 1.0),
            );
            lerp(fade(d3), lerp(fade(d2), x0, x1), lerp(fade(d2), x2, x3))
        };
        const F: f64 = 337.0 / 331.0;
        let stack = |lane: usize, x: f64, y: f64, z: f64| {
            let mut v = 0.0;
            for o in noise.octaves.iter() {
                let lf = o.lacunarity;
                v += o.amplitude * octave(o, lane, x * lf, y * lf, z * lf);
            }
            v
        };
        (stack(0, x, y, z) + stack(1, x * F, y * F, z * F)) * noise.amplitude
    }

    #[test]
    fn lane_evaluation_matches_the_scalar_double_perlin_bit_for_bit() {
        let fields = [
            build_climate_field(0x1234_5678, &climate_fields::CONTINENTALITY),
            build_climate_field(0xDEAD_BEEF, &climate_fields::TEMPERATURE),
            ReferenceDoublePerlin::from_seed(7, [3, 9], -8, &[0.5, 1.0, 2.0, 0.0, 2.0]),
        ];
        let mut rng = Xoroshiro::new(42);
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        for i in 0..40_000u32 {
            let scale = [1.0, 300.0, 1.0e6, 4.0e9, 1.0e14, 1.0e300][(i % 6) as usize];
            let mut p = [0.0; 3].map(|_| (rng.next_double() - 0.5) * scale);
            match i % 9 {
                0 => p[1] = 0.0,
                1 => p[1] = -0.0,
                2 => p = p.map(f64::round),
                3 => p[0] = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY][(i / 9 % 3) as usize],
                _ => {}
            }
            for field in &fields {
                let want = scalar_sample(field, p[0], p[1], p[2]);
                let got = field.sample(p[0], p[1], p[2]);
                let portable = field.sample_lanes::<lanes::Portable>(p[0], p[1], p[2]);
                assert!(same(want, got), "{p:?}: {want} vs {got}");
                assert!(same(want, portable), "{p:?}: {want} vs portable {portable}");
            }
        }
    }

    #[test]
    fn batched_kernels_match_the_scalar_double_perlin_bit_for_bit() {
        let fields = [
            build_climate_field(0x1234_5678, &climate_fields::CONTINENTALITY),
            ReferenceDoublePerlin::from_seed(7, [3, 9], -8, &[0.5, 1.0, 2.0, 0.0, 2.0]),
            ReferenceDoublePerlin::from_seed(11, [5, 1], -4, &[1.0]),
        ];
        let mut rng = Xoroshiro::new(99);
        let same = |a: f64, b: f64| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        for round in 0..400u32 {
            // Odd lengths exercise the four-lane kernel's lone tail point;
            // clustered points exercise corner-hash reuse.
            let n = 1 + (round as usize * 7) % 131;
            let scale = [4.0, 300.0, 1.0e6, 4.0e9, 1.0e300][(round % 5) as usize];
            let base = [0.0; 3].map(|_| (rng.next_double() - 0.5) * scale);
            let mut pts: Vec<[f64; 3]> = (0..n)
                .map(|i| {
                    let step = if round % 2 == 0 { 0.37 } else { 97.0 };
                    [
                        base[0] + i as f64 * step,
                        base[1],
                        base[2] - i as f64 * step,
                    ]
                })
                .collect();
            if round % 3 == 0 {
                pts[n / 2] = [f64::NAN, -0.0, f64::INFINITY];
            }
            let (x, y, z): (Vec<_>, Vec<_>, Vec<_>) = (
                pts.iter().map(|p| p[0]).collect(),
                pts.iter().map(|p| p[1]).collect(),
                pts.iter().map(|p| p[2]).collect(),
            );
            for field in &fields {
                let mut narrow = vec![0.0; n];
                let mut wide = vec![0.0; n];
                field.sample_many_with(false, &x, &y, &z, &mut narrow);
                field.sample_many_with(true, &x, &y, &z, &mut wide);
                let mut portable = vec![0.0; n];
                for start in (0..n).step_by(64) {
                    let end = (start + 64).min(n);
                    field.sample_chunk::<lanes::Portable>(
                        &x[start..end],
                        &y[start..end],
                        &z[start..end],
                        &mut portable[start..end],
                    );
                }
                for i in 0..n {
                    let want = scalar_sample(field, x[i], y[i], z[i]);
                    assert!(
                        same(want, narrow[i]),
                        "{:?}: {want} vs two-lane {}",
                        pts[i],
                        narrow[i]
                    );
                    assert!(
                        same(want, wide[i]),
                        "{:?}: {want} vs four-lane {}",
                        pts[i],
                        wide[i]
                    );
                    assert!(
                        same(want, portable[i]),
                        "{:?}: {want} vs portable {}",
                        pts[i],
                        portable[i]
                    );
                }
            }
        }
    }

    #[test]
    fn perlin_gradients_have_root_two_magnitude() {
        for (x, y, z) in lanes::gradients() {
            let mag_sq = x * x + y * y + z * z;
            assert!(
                (mag_sq - 2.0).abs() < 1.0e-12,
                "gradient {:?} must have magnitude √2 (squared = 2), got {mag_sq}",
                (x, y, z)
            );
        }
    }
}
