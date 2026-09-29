//! Two f64 lanes for evaluating a double-Perlin's two octave stacks side by
//! side. Every operation is the scalar operation per lane, in the same order,
//! so a lane's result is bit-identical to the scalar evaluation.

const GX: [f64; 16] = [
    1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0,
];
const GY: [f64; 16] = [
    1.0, 1.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0,
];
const GZ: [f64; 16] = [
    0.0, 0.0, 0.0, 0.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 0.0, 1.0, 0.0, -1.0,
];

#[cfg(test)]
pub(super) fn gradients() -> impl Iterator<Item = (f64, f64, f64)> {
    (0..16).map(|i| (GX[i], GY[i], GZ[i]))
}

/// The gradient of both lanes at once, indexed `(lane0_hash & 15) << 4 |
/// (lane1_hash & 15)`: `[[gx0, gx1], [gy0, gy1], [gz0, gz1]]`.
pub(super) static GRADIENT_PAIRS: [[[f64; 2]; 3]; 256] = {
    let mut out = [[[0.0; 2]; 3]; 256];
    let mut i = 0;
    while i < 256 {
        let (a, b) = (i >> 4, i & 15);
        out[i] = [[GX[a], GX[b]], [GY[a], GY[b]], [GZ[a], GZ[b]]];
        i += 1;
    }
    out
};

/// The pair index of two lanes' gradient hashes.
#[inline(always)]
pub(super) fn gradient_pair(a: u8, b: u8) -> u8 {
    ((a & 15) << 4) | (b & 15)
}

pub(super) trait Lanes:
    Copy + std::ops::Add<Output = Self> + std::ops::Sub<Output = Self> + std::ops::Mul<Output = Self>
{
    fn new(a: f64, b: f64) -> Self;
    fn load(pair: &[f64; 2]) -> Self;
    fn splat(v: f64) -> Self;
    fn to_array(self) -> [f64; 2];
    /// `(floor(v), floor(v) as i64 as u8)` per lane.
    fn floor_hash(self) -> (Self, [u8; 2]);

    #[inline(always)]
    fn fade(self) -> Self {
        let t = self;
        t * t * t * (t * (t * Self::splat(6.0) - Self::splat(15.0)) + Self::splat(10.0))
    }

    #[inline(always)]
    fn lerp(part: Self, from: Self, to: Self) -> Self {
        from + part * (to - from)
    }

    /// The dot product of both lanes' gradients (see [`gradient_pair`]).
    #[inline(always)]
    fn grad(pair: u8, dx: Self, dy: Self, dz: Self) -> Self {
        let [gx, gy, gz] = &GRADIENT_PAIRS[usize::from(pair)];
        Self::load(gx) * dx + Self::load(gy) * dy + Self::load(gz) * dz
    }
}

#[inline(always)]
fn floor_hash_scalar(v: f64) -> (f64, u8) {
    let floor = super::floor(v);
    (floor, floor as i64 as u8)
}

#[cfg(any(test, not(target_arch = "x86_64")))]
#[derive(Clone, Copy, Debug)]
pub(super) struct Portable([f64; 2]);

#[cfg(any(test, not(target_arch = "x86_64")))]
impl Lanes for Portable {
    #[inline(always)]
    fn new(a: f64, b: f64) -> Self {
        Self([a, b])
    }
    #[inline(always)]
    fn load(pair: &[f64; 2]) -> Self {
        Self(*pair)
    }
    #[inline(always)]
    fn splat(v: f64) -> Self {
        Self([v; 2])
    }
    #[inline(always)]
    fn to_array(self) -> [f64; 2] {
        self.0
    }
    #[inline(always)]
    fn floor_hash(self) -> (Self, [u8; 2]) {
        let (a, ha) = floor_hash_scalar(self.0[0]);
        let (b, hb) = floor_hash_scalar(self.0[1]);
        (Self([a, b]), [ha, hb])
    }
}

macro_rules! lane_ops {
    ($t:ty, $add:expr, $sub:expr, $mul:expr) => {
        impl std::ops::Add for $t {
            type Output = $t;
            #[inline(always)]
            fn add(self, o: $t) -> $t {
                $add(self, o)
            }
        }
        impl std::ops::Sub for $t {
            type Output = $t;
            #[inline(always)]
            fn sub(self, o: $t) -> $t {
                $sub(self, o)
            }
        }
        impl std::ops::Mul for $t {
            type Output = $t;
            #[inline(always)]
            fn mul(self, o: $t) -> $t {
                $mul(self, o)
            }
        }
    };
}

#[cfg(any(test, not(target_arch = "x86_64")))]
lane_ops!(
    Portable,
    |a: Portable, b: Portable| Portable([a.0[0] + b.0[0], a.0[1] + b.0[1]]),
    |a: Portable, b: Portable| Portable([a.0[0] - b.0[0], a.0[1] - b.0[1]]),
    |a: Portable, b: Portable| Portable([a.0[0] * b.0[0], a.0[1] * b.0[1]])
);

#[cfg(target_arch = "x86_64")]
mod sse2 {
    use super::Lanes;
    use std::arch::x86_64::*;

    /// SSE2 is part of the x86-64 baseline, so no runtime detection is needed.
    #[derive(Clone, Copy, Debug)]
    pub(in super::super) struct Sse2(__m128d);

    impl Lanes for Sse2 {
        #[inline(always)]
        fn new(a: f64, b: f64) -> Self {
            // SAFETY: SSE2 is always available on x86_64.
            unsafe { Self(_mm_set_pd(b, a)) }
        }
        #[inline(always)]
        fn load(pair: &[f64; 2]) -> Self {
            // SAFETY: SSE2 is always available; `pair` holds two f64s.
            unsafe { Self(_mm_loadu_pd(pair.as_ptr())) }
        }
        #[inline(always)]
        fn splat(v: f64) -> Self {
            // SAFETY: SSE2 is always available on x86_64.
            unsafe { Self(_mm_set1_pd(v)) }
        }
        #[inline(always)]
        fn to_array(self) -> [f64; 2] {
            let mut out = [0.0; 2];
            // SAFETY: SSE2 is always available; `out` holds two f64s.
            unsafe { _mm_storeu_pd(out.as_mut_ptr(), self.0) };
            out
        }
        #[inline(always)]
        fn floor_hash(self) -> (Self, [u8; 2]) {
            // SAFETY: SSE2 is always available on x86_64.
            unsafe {
                let sign = _mm_set1_pd(-0.0);
                let x = self.0;
                let small = _mm_cmplt_pd(_mm_andnot_pd(sign, x), _mm_set1_pd(1_073_741_824.0));
                if _mm_movemask_pd(small) != 3 {
                    let [a, b] = self.to_array();
                    let (a, ha) = super::floor_hash_scalar(a);
                    let (b, hb) = super::floor_hash_scalar(b);
                    return (Self::new(a, b), [ha, hb]);
                }
                // In range, truncation is exact; stepping down where it rounded
                // up gives the floor, and OR-ing the input's sign bit reproduces
                // `copysign` for -0.0 without disturbing any other value.
                let t = _mm_cvtepi32_pd(_mm_cvttpd_epi32(x));
                let up = _mm_and_pd(_mm_cmpgt_pd(t, x), _mm_set1_pd(1.0));
                let floor = _mm_or_pd(_mm_sub_pd(t, up), _mm_and_pd(x, sign));
                let ints = _mm_cvttpd_epi32(floor);
                let lo = _mm_cvtsi128_si32(ints) as u8;
                let hi = _mm_cvtsi128_si32(_mm_srli_si128::<4>(ints)) as u8;
                (Self(floor), [lo, hi])
            }
        }
    }

    lane_ops!(
        Sse2,
        // SAFETY: SSE2 is always available on x86_64.
        |a: Sse2, b: Sse2| unsafe { Sse2(_mm_add_pd(a.0, b.0)) },
        |a: Sse2, b: Sse2| unsafe { Sse2(_mm_sub_pd(a.0, b.0)) },
        |a: Sse2, b: Sse2| unsafe { Sse2(_mm_mul_pd(a.0, b.0)) }
    );
}

#[cfg(target_arch = "x86_64")]
pub(super) type F2 = sse2::Sse2;
#[cfg(not(target_arch = "x86_64"))]
pub(super) type F2 = Portable;
