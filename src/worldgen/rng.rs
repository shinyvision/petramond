//! Deterministic feature RNG.
//!
//! The xorshift64 *stream stepper* (`next_*`) is unchanged across all phases.
//! Two seedings exist: `new` keys the stream to a chunk (used through P3 for
//! byte-parity); `positional` keys it to a world cell via a splitmix64
//! finalizer (P4), so a feature draws an identical stream regardless of which
//! chunk/worker/order generates it — the precondition for cross-chunk replay.
//! Only the seed derivation differs; `next_*` semantics are identical.

/// xorshift64 RNG seeded deterministically from world seed + chunk pos.
pub struct FeatureRng {
    state: u64,
}
impl FeatureRng {
    pub fn new(seed: u32, cx: i32, cz: i32) -> Self {
        let mut s = seed as u64
            ^ ((cx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
            ^ ((cz as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        if s == 0 {
            s = 0xDEAD_BEEF;
        }
        Self { state: s }
    }

    /// Construct directly from a stream state (zero-guarded).
    #[inline]
    pub fn from_state(state: u64) -> Self {
        Self {
            state: if state == 0 { 0xDEAD_BEEF } else { state },
        }
    }

    /// Positional seeding: mix (seed, salt, world coords) with a splitmix64
    /// finalizer, then step the same xorshift64 stream. Pure function of the
    /// inputs, bit-identical across platforms (all `wrapping` u64 ops).
    pub fn positional(seed: u32, salt: u64, wx: i32, wy: i32, wz: i32) -> Self {
        let mut z = (seed as u64)
            ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (wx as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
            ^ (wy as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
            ^ (wz as i64 as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self::from_state(z)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn next_i32(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as i32
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.next_f32() < p
    }
}
