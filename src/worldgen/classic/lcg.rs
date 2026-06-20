//! A 48-bit linear congruential generator (LCG) — the deterministic random
//! source the classic world generator draws from for every decision (biome layer
//! seeds, terrain noise construction, and decoration placement).
//!
//! The output is pinned bit-for-bit to a well-known 48-bit LCG parameterization
//! so generation is a stable, exactly-reproducible function of the seed. The
//! known-answer vectors in the tests lock that output: any change to the algorithm
//! breaks them, which guards determinism across refactors and platforms (x86-64
//! and wasm32 produce identical streams — all ops are `wrapping` on `u64`).

/// LCG multiplier. Standard, well-tested 48-bit LCG parameterization.
const MULTIPLIER: u64 = 0x5DEECE66D;
/// LCG increment.
const ADDEND: u64 = 0xB;
/// 48-bit state mask.
const MASK: u64 = (1 << 48) - 1;

/// A 48-bit LCG random source with the classic `next(bits)` extraction and the
/// derived `next_int`/`next_int_bound`/`next_long`/`next_float`/`next_double`/
/// `next_boolean` helpers. Cheap to clone; holds only the 48-bit state.
#[derive(Clone, Debug)]
pub struct LcgRandom {
    seed: u64,
}

impl LcgRandom {
    /// Construct from a seed, applying the initial-scramble XOR with the
    /// multiplier (so nearby seeds don't produce correlated early output).
    #[inline]
    pub fn new(seed: i64) -> Self {
        Self {
            seed: (seed as u64 ^ MULTIPLIER) & MASK,
        }
    }

    /// Re-seed in place, same scramble as the constructor.
    #[inline]
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed as u64 ^ MULTIPLIER) & MASK;
    }

    /// Advance the LCG and return the top `bits` bits (`bits <= 32`) as a signed
    /// 32-bit int. The `as i32` truncation reinterprets the low 32 bits as signed.
    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        (self.seed >> (48 - bits)) as i32
    }

    /// A full signed 32-bit int.
    #[inline]
    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// Uniform in `[0, bound)`. Powers of two use the high-bits fast path;
    /// otherwise rejection-sample to avoid modulo bias (the `bits - val +
    /// (bound-1) < 0` overflow retry, in wrapping i32 math).
    #[inline]
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        debug_assert!(bound > 0, "bound must be positive");
        if (bound & bound.wrapping_neg()) == bound {
            // power of two: (bound * next(31)) >> 31
            return ((bound as i64 * self.next(31) as i64) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }

    /// A signed 64-bit int: `(next(32) << 32) + next(32)`, high word first.
    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let hi = (self.next(32) as i64) << 32;
        hi.wrapping_add(self.next(32) as i64)
    }

    /// A float in `[0, 1)`: `next(24) / 2^24`.
    #[inline]
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1u32 << 24) as f32
    }

    /// A double in `[0, 1)`: `((next(26) << 27) + next(27)) * 2^-53`.
    #[inline]
    pub fn next_double(&mut self) -> f64 {
        let hi = (self.next(26) as i64) << 27;
        let lo = self.next(27) as i64;
        hi.wrapping_add(lo) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A boolean: `next(1) != 0`.
    #[inline]
    pub fn next_boolean(&mut self) -> bool {
        self.next(1) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known-answer vectors: the canonical outputs of this 48-bit LCG for seed 0
    // (independently reproducible). They pin the generator to exact, byte-stable
    // behaviour — the verification anchor every higher layer builds on.

    #[test]
    fn next_int_sequence_for_seed_zero() {
        let mut r = LcgRandom::new(0);
        assert_eq!(r.next_int(), -1_155_484_576);
        assert_eq!(r.next_int(), -723_955_400);
        assert_eq!(r.next_int(), 1_033_096_058);
    }

    #[test]
    fn next_long_for_seed_zero() {
        let mut r = LcgRandom::new(0);
        assert_eq!(r.next_long(), -4_962_768_465_676_381_896);
    }

    #[test]
    fn next_double_for_seed_zero() {
        let mut r = LcgRandom::new(0);
        assert!((r.next_double() - 0.730_967_787_376_657).abs() < 1e-15);
    }

    #[test]
    fn next_float_for_seed_zero() {
        let mut r = LcgRandom::new(0);
        assert!((r.next_float() - 0.730_967_77).abs() < 1e-6);
    }

    #[test]
    fn next_boolean_for_seed_zero() {
        let mut r = LcgRandom::new(0);
        assert!(r.next_boolean());
    }

    #[test]
    fn next_int_bound_non_power_of_two() {
        // first next(31) for fresh new(0) is 1_569_741_360; 1569741360 % 100 == 60.
        let mut r = LcgRandom::new(0);
        assert_eq!(r.next_int_bound(100), 60);
    }

    #[test]
    fn next_int_bound_power_of_two() {
        // (16 * 1_569_741_360) >> 31 == 11.
        let mut r = LcgRandom::new(0);
        assert_eq!(r.next_int_bound(16), 11);
    }

    #[test]
    fn next_int_bound_is_in_range() {
        let mut r = LcgRandom::new(123_456_789);
        for bound in [1, 2, 3, 7, 16, 100, 256, 1000] {
            for _ in 0..1000 {
                let v = r.next_int_bound(bound);
                assert!((0..bound).contains(&v), "out of range for bound {bound}");
            }
        }
    }

    #[test]
    fn set_seed_matches_constructor() {
        let mut a = LcgRandom::new(0xCAFE_BABE);
        let mut b = LcgRandom::new(0);
        b.set_seed(0xCAFE_BABE);
        for _ in 0..50 {
            assert_eq!(a.next_int(), b.next_int());
        }
    }
}
