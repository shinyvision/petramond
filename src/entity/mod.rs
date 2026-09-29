mod dropped_item;
#[cfg(test)]
pub mod fluid_fixture;
mod item_rules;
pub mod shore;

#[cfg(any(test, feature = "test-support"))]
pub use dropped_item::ATTRACT_RADIUS;
pub use dropped_item::{DroppedItem, Fate, Flight, Heading, Motion, Stuck};

pub const VELOCITY_SLACK: f32 = 1.25;

#[inline]
pub fn hash01(seed: u64) -> f32 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 40) as f32) / ((1u32 << 24) as f32)
}

#[inline]
pub fn hash_signed(seed: u64) -> f32 {
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
