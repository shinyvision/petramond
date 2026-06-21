//! Client-side entities: dropped item-stacks and short-lived particles.
//!
//! Owned by `App` (never `World`), so they stay off the worker threads and out
//! of the chunk save path. Ticked in `App::tick` between `world.poll()` and
//! `tick_mesh_budget`.
//!
//! **Render-agnostic rule:** nothing here may depend on `crate::render`. The
//! module exposes raw [`DroppedItem`] / [`Particle`] data (slices/accessors) and
//! a single [`Particle::atlas_uv`] helper that resolves absolute atlas UVs via
//! [`crate::atlas`]; the App maps these to render instances in a later layer.

mod dropped_item;
mod particle;

pub use dropped_item::{DroppedItem, ABSORB_RADIUS, ATTRACT_RADIUS, GRAVITY};
pub use particle::{Particle, ParticleSystem, PARTICLE_CAPACITY};

/// A tiny deterministic hash → `f32` in `[0, 1)`. Replaces an RNG so spawns are
/// reproducible and we never pull in the `rand` crate (banned in workflow
/// scripts; unnecessary for this much variety). A SplitMix64-style finalizer
/// gives good bit avalanche from a small incrementing counter.
#[inline]
pub(crate) fn hash01(seed: u64) -> f32 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // Top 24 bits → a uniform float in [0, 1). 24 bits is the f32 mantissa.
    ((z >> 40) as f32) / ((1u32 << 24) as f32)
}

/// Symmetric variant of [`hash01`]: a deterministic value in `[-1, 1)`.
#[inline]
pub(crate) fn hash_signed(seed: u64) -> f32 {
    hash01(seed) * 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash01_is_in_unit_range_and_deterministic() {
        for i in 0..10_000u64 {
            let h = hash01(i);
            assert!((0.0..1.0).contains(&h), "hash01({i}) = {h} out of range");
            assert_eq!(h, hash01(i), "hash01 must be deterministic");
        }
    }

    #[test]
    fn hash01_spreads_across_the_range() {
        // Crude uniformity check: every decile bucket sees at least one sample.
        let mut buckets = [0u32; 10];
        for i in 0..10_000u64 {
            let b = (hash01(i.wrapping_mul(2_654_435_761)) * 10.0) as usize;
            buckets[b.min(9)] += 1;
        }
        assert!(buckets.iter().all(|&c| c > 0), "buckets: {buckets:?}");
    }

    #[test]
    fn hash_signed_is_symmetric_range() {
        for i in 0..10_000u64 {
            let h = hash_signed(i);
            assert!((-1.0..1.0).contains(&h), "hash_signed({i}) = {h}");
        }
    }
}
