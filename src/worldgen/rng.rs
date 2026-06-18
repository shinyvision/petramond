//! Deterministic feature RNG.
//!
//! Strata P0: relocated verbatim from `gen.rs` `mod rng`. The xorshift64 stream
//! and per-chunk seed derivation are byte-identical through P3; P4 adds a
//! positional (world-coordinate-seeded) variant alongside this one.

/// xorshift64 RNG seeded deterministically from world seed + chunk pos.
pub struct FeatureRng { state: u64 }
impl FeatureRng {
    pub fn new(seed: u32, cx: i32, cz: i32) -> Self {
        let mut s = seed as u64
            ^ ((cx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            ^ ((cz as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        if s == 0 { s = 0xDEAD_BEEF; }
        Self { state: s }
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.state = x; x
    }
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn next_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as i32
    }
    pub fn chance(&mut self, p: f32) -> bool { self.next_f32() < p }
}
