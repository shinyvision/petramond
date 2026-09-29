//! [`ReferenceDoublePerlin::sample_many`] on four lanes: two points, both
//! octave stacks each. Every lane performs the scalar evaluation's operations
//! in the same order, so results are bit-identical to the two-lane path.

use std::arch::x86_64::*;

use super::lanes::{Lanes, GRADIENT_PAIRS};
use super::{OctavePair, ReferenceDoublePerlin, F2};

const F: f64 = 337.0 / 331.0;

#[inline]
#[target_feature(enable = "avx2")]
fn fade(t: __m256d) -> __m256d {
    let inner = _mm256_add_pd(
        _mm256_mul_pd(
            t,
            _mm256_sub_pd(_mm256_mul_pd(t, _mm256_set1_pd(6.0)), _mm256_set1_pd(15.0)),
        ),
        _mm256_set1_pd(10.0),
    );
    _mm256_mul_pd(_mm256_mul_pd(_mm256_mul_pd(t, t), t), inner)
}

#[inline]
#[target_feature(enable = "avx2")]
fn lerp(t: __m256d, from: __m256d, to: __m256d) -> __m256d {
    _mm256_add_pd(from, _mm256_mul_pd(t, _mm256_sub_pd(to, from)))
}

#[inline]
#[target_feature(enable = "avx2")]
fn grad(lo: u8, hi: u8, dx: __m256d, dy: __m256d, dz: __m256d) -> __m256d {
    let (a, b) = (
        &GRADIENT_PAIRS[usize::from(lo)],
        &GRADIENT_PAIRS[usize::from(hi)],
    );
    // SAFETY: each `[f64; 2]` row is 16 readable bytes.
    let load = |k: usize| unsafe { _mm256_loadu2_m128d(b[k].as_ptr(), a[k].as_ptr()) };
    _mm256_add_pd(
        _mm256_add_pd(_mm256_mul_pd(load(0), dx), _mm256_mul_pd(load(1), dy)),
        _mm256_mul_pd(load(2), dz),
    )
}

#[inline]
#[target_feature(enable = "avx2")]
fn blend(g: &[u8; 8], h: &[u8; 8], [d1, d2, d3]: [__m256d; 3]) -> __m256d {
    let (t1, t2, t3) = (fade(d1), fade(d2), fade(d3));
    let one = _mm256_set1_pd(1.0);
    let (e1, e2, e3) = (
        _mm256_sub_pd(d1, one),
        _mm256_sub_pd(d2, one),
        _mm256_sub_pd(d3, one),
    );
    let l1 = grad(g[0], h[0], d1, d2, d3);
    let l2 = grad(g[1], h[1], e1, d2, d3);
    let l3 = grad(g[2], h[2], d1, e2, d3);
    let l4 = grad(g[3], h[3], e1, e2, d3);
    let l5 = grad(g[4], h[4], d1, d2, e3);
    let l6 = grad(g[5], h[5], e1, d2, e3);
    let l7 = grad(g[6], h[6], d1, e2, e3);
    let l8 = grad(g[7], h[7], e1, e2, e3);
    let l1 = lerp(t1, l1, l2);
    let l3 = lerp(t1, l3, l4);
    let l5 = lerp(t1, l5, l6);
    let l7 = lerp(t1, l7, l8);
    let l1 = lerp(t2, l1, l3);
    let l5 = lerp(t2, l5, l7);
    lerp(t3, l1, l5)
}

/// One octave's contribution for points `2j` and `2j + 1` through the
/// two-lane path, for coordinates outside the fast path's range.
fn fallback(octave: &OctavePair, p: [[f64; 3]; 2]) -> [f64; 4] {
    let lf = F2::splat(octave.lacunarity);
    let one = |[x, y, z]: [f64; 3]| {
        let (x, y, z) = (F2::new(x, x * F), F2::new(y, y * F), F2::new(z, z * F));
        octave.sample(x * lf, y * lf, z * lf).to_array()
    };
    let ([a, b], [c, d]) = (one(p[0]), one(p[1]));
    [a, b, c, d]
}

/// # Safety
/// The CPU must support AVX2.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn sample_chunk(
    noise: &ReferenceDoublePerlin,
    x: &[f64],
    y: &[f64],
    z: &[f64],
    out: &mut [f64],
) {
    let n = out.len();
    debug_assert!(n <= 64 && x.len() == n && y.len() == n && z.len() == n);
    let pairs = n / 2;
    let zero = _mm256_setzero_pd();
    let mut p = [[zero; 3]; 32];
    let mut acc = [zero; 32];
    for (j, pair) in p.iter_mut().enumerate().take(pairs) {
        let (i, k) = (2 * j, 2 * j + 1);
        *pair = [
            _mm256_set_pd(x[k] * F, x[k], x[i] * F, x[i]),
            _mm256_set_pd(y[k] * F, y[k], y[i] * F, y[i]),
            _mm256_set_pd(z[k] * F, z[k], z[i] * F, z[i]),
        ];
    }
    let sign = _mm256_set1_pd(-0.0);
    let limit = _mm256_set1_pd(1_073_741_824.0);
    let low_bytes = _mm_setr_epi8(0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
    for octave in noise.octaves.iter() {
        let lf = _mm256_set1_pd(octave.lacunarity);
        let amp = _mm256_set1_pd(octave.amplitude);
        let o = &octave.origin;
        let ox = _mm256_set_pd(o[0][1], o[0][0], o[0][1], o[0][0]);
        let oy = _mm256_set_pd(o[1][1], o[1][0], o[1][1], o[1][0]);
        let oz = _mm256_set_pd(o[2][1], o[2][0], o[2][1], o[2][0]);
        let mut last = None;
        let mut g = [0u8; 8];
        for j in 0..pairs {
            let dx = _mm256_add_pd(_mm256_mul_pd(p[j][0], lf), ox);
            let dy = _mm256_add_pd(_mm256_mul_pd(p[j][1], lf), oy);
            let dz = _mm256_add_pd(_mm256_mul_pd(p[j][2], lf), oz);
            let in_range = |v: __m256d| {
                _mm256_movemask_pd(_mm256_cmp_pd::<_CMP_LT_OQ>(
                    _mm256_andnot_pd(sign, v),
                    limit,
                ))
            };
            let contribution = if in_range(dx) & in_range(dy) & in_range(dz) != 0xf {
                let mut v = [[0.0; 4]; 3];
                // SAFETY: each row holds four f64s.
                unsafe {
                    _mm256_storeu_pd(v[0].as_mut_ptr(), p[j][0]);
                    _mm256_storeu_pd(v[1].as_mut_ptr(), p[j][1]);
                    _mm256_storeu_pd(v[2].as_mut_ptr(), p[j][2]);
                }
                let c = fallback(
                    octave,
                    [[v[0][0], v[1][0], v[2][0]], [v[0][2], v[1][2], v[2][2]]],
                );
                _mm256_set_pd(c[3], c[2], c[1], c[0])
            } else {
                let (fx, fy, fz) = (
                    _mm256_floor_pd(dx),
                    _mm256_floor_pd(dy),
                    _mm256_floor_pd(dz),
                );
                let bytes = |f: __m256d| {
                    let ints = _mm_shuffle_epi8(_mm256_cvttpd_epi32(f), low_bytes);
                    (_mm_cvtsi128_si32(ints) as u32).to_le_bytes()
                };
                let (hx, hy, hz) = (bytes(fx), bytes(fy), bytes(fz));
                let first = [[hx[0], hx[1]], [hy[0], hy[1]], [hz[0], hz[1]]];
                let second = [[hx[2], hx[3]], [hy[2], hy[3]], [hz[2], hz[3]]];
                if last != Some(first) {
                    g = octave.corners(first);
                }
                let h = if second == first {
                    g
                } else {
                    octave.corners(second)
                };
                last = Some(second);
                let frac = [
                    _mm256_sub_pd(dx, fx),
                    _mm256_sub_pd(dy, fy),
                    _mm256_sub_pd(dz, fz),
                ];
                let result = blend(&g, &h, frac);
                g = h;
                result
            };
            acc[j] = _mm256_add_pd(acc[j], _mm256_mul_pd(amp, contribution));
        }
    }
    for j in 0..pairs {
        let mut v = [0.0; 4];
        // SAFETY: `v` holds four f64s.
        unsafe { _mm256_storeu_pd(v.as_mut_ptr(), acc[j]) };
        out[2 * j] = (v[0] + v[1]) * noise.amplitude;
        out[2 * j + 1] = (v[2] + v[3]) * noise.amplitude;
    }
    if n % 2 == 1 {
        out[n - 1] = noise.sample(x[n - 1], y[n - 1], z[n - 1]);
    }
}
