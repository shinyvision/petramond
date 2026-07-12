//! Deterministic positional RNG for discrete worldgen work.
//!
//! `FeatureRng::positional(world_seed, salt, wx, wy, wz)` is the frozen contract:
//! derive every independent stream from world seed, signed world coordinates,
//! and a per-purpose salt. Never draw worldgen decisions from a shared mutable
//! stream whose state depends on chunk visit order.
//!
//! The positional mix is a SplitMix64 finalizer over those inputs; the stream
//! stepper is xorshift64. Changing either is a worldgen compatibility break and
//! requires updating the pinned vectors below.

const SALT_MULTIPLIER: u64 = 0x9E37_79B9_7F4A_7C15;
const X_MULTIPLIER: u64 = 0xC2B2_AE3D_27D4_EB4F;
const Y_MULTIPLIER: u64 = 0x1656_67B1_9E37_79F9;
const Z_MULTIPLIER: u64 = 0xD6E8_FEB8_6659_FD93;
const SPLITMIX_MIX_1: u64 = 0xBF58_476D_1CE4_E5B9;
const SPLITMIX_MIX_2: u64 = 0x94D0_49BB_1331_11EB;
const ZERO_STREAM_STATE: u64 = 0xDEAD_BEEF;

#[inline]
fn non_zero_stream_state(state: u64) -> u64 {
    if state == 0 {
        ZERO_STREAM_STATE
    } else {
        state
    }
}

#[inline]
fn positional_state(seed: u32, salt: u64, wx: i32, wy: i32, wz: i32) -> u64 {
    let mut z = (seed as u64)
        ^ salt.wrapping_mul(SALT_MULTIPLIER)
        ^ (wx as i64 as u64).wrapping_mul(X_MULTIPLIER)
        ^ (wy as i64 as u64).wrapping_mul(Y_MULTIPLIER)
        ^ (wz as i64 as u64).wrapping_mul(Z_MULTIPLIER);
    z = (z ^ (z >> 30)).wrapping_mul(SPLITMIX_MIX_1);
    z = (z ^ (z >> 27)).wrapping_mul(SPLITMIX_MIX_2);
    non_zero_stream_state(z ^ (z >> 31))
}

/// xorshift64 RNG seeded deterministically from world seed + position + salt.
/// `Copy` is deliberate: pre-placement probes (e.g. the oak anchoring gate)
/// dry-run a feature's draw prefix on a copy so the real placement still sees
/// the unconsumed stream.
#[derive(Clone, Copy)]
pub struct FeatureRng {
    state: u64,
}
impl FeatureRng {
    /// Construct directly from a stream state (zero-guarded).
    #[inline]
    pub fn from_state(state: u64) -> Self {
        Self {
            state: non_zero_stream_state(state),
        }
    }

    /// Positional seeding: mix (seed, salt, world coords) with a splitmix64
    /// finalizer, then step the same xorshift64 stream. Pure function of the
    /// inputs, bit-identical across platforms (all `wrapping` u64 ops).
    pub fn positional(seed: u32, salt: u64, wx: i32, wy: i32, wz: i32) -> Self {
        Self::from_state(positional_state(seed, salt, wx, wy, wz))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn stream_prefix(seed: u32, salt: u64, wx: i32, wy: i32, wz: i32) -> [u64; 4] {
        let mut rng = FeatureRng::positional(seed, salt, wx, wy, wz);
        [
            rng.next_u64(),
            rng.next_u64(),
            rng.next_u64(),
            rng.next_u64(),
        ]
    }

    #[test]
    fn positional_stream_vectors_are_frozen() {
        let cases = [
            (
                (0x0000_0000, 0x0000_0000_0000_0000, 0, 0, 0),
                [
                    0x37c5_9ca7_bf06_be52,
                    0x167a_05ab_2941_67ae,
                    0xaae6_f93d_9e7d_cee1,
                    0xe5e5_4fba_9996_ad3c,
                ],
            ),
            (
                (0x1234_5678, 0x0000_7a3e_0ac0_ffee, 12, 0, -34),
                [
                    0x6ac6_a985_c496_4f45,
                    0x44e3_bbfd_0652_129b,
                    0x75f9_7613_ca75_707e,
                    0xa90a_c427_548e_451e,
                ],
            ),
            (
                (0xdead_beef, 0x9e37_79b9_7f4a_7c15, -1, -64, i32::MIN),
                [
                    0xee3d_9ace_03ab_f28f,
                    0x79c2_840d_c655_e6aa,
                    0x5c6c_5255_bbbb_a7e7,
                    0x865b_bd5b_1809_c968,
                ],
            ),
            (
                (u32::MAX, u64::MAX, i32::MAX, 255, i32::MIN),
                [
                    0xe484_0ea8_eb4d_12c6,
                    0x0656_26e9_3941_f963,
                    0xec8b_1f00_00c3_4251,
                    0x1c39_7996_b972_7095,
                ],
            ),
        ];

        for ((seed, salt, wx, wy, wz), expected) in cases {
            assert_eq!(
                stream_prefix(seed, salt, wx, wy, wz),
                expected,
                "positional stream changed for seed {seed:#x}, salt {salt:#x}, pos ({wx},{wy},{wz})"
            );
        }
    }

    #[test]
    fn positional_inputs_select_distinct_streams() {
        let inputs = [
            (0x1234_5678, 0x0000_7a3e_0ac0_ffee, 4, 5, 6),
            (0x1234_5678, 0x0000_7a3e_0ac0_ffef, 4, 5, 6),
            (0x1234_5678, 0x0000_7a3e_0ac0_ffee, 5, 5, 6),
            (0x1234_5678, 0x0000_7a3e_0ac0_ffee, 4, 6, 6),
            (0x1234_5678, 0x0000_7a3e_0ac0_ffee, 4, 5, 7),
        ];
        let streams = inputs
            .into_iter()
            .map(|(seed, salt, wx, wy, wz)| stream_prefix(seed, salt, wx, wy, wz))
            .collect::<HashSet<_>>();

        assert_eq!(
            streams.len(),
            inputs.len(),
            "salt and each coordinate axis must contribute to positional seeding"
        );
    }

    #[test]
    fn repeated_positional_construction_replays_the_same_stream() {
        let input = (0x5eed_1234, 0xf00d_cafe_dead_beef, -19, 83, 41);
        let mut first = FeatureRng::positional(input.0, input.1, input.2, input.3, input.4);

        let consumed = [
            first.next_u64(),
            first.next_u64(),
            first.next_u64(),
            first.next_u64(),
        ];

        assert_eq!(
            consumed,
            stream_prefix(input.0, input.1, input.2, input.3, input.4),
            "constructing a positional RNG must not depend on prior stream consumption"
        );
    }
}
