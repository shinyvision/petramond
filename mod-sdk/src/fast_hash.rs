//! A fast non-cryptographic hasher for a mod's own maps.
//!
//! `std`'s default SipHash defends against hostile keys; a mod hashing cells,
//! ids and indices by the thousand per tick pays for a defence it has no use
//! for. [`FxHashMap`] / [`FxHashSet`] are the `std` collections over the Fx
//! word hash: same API, `::default()` instead of `::new()`.
//!
//! Iteration order is as unspecified as `std`'s, but it is the SAME on every
//! run (no random seed), so it never breaks determinism on its own.

use std::hash::{BuildHasherDefault, Hasher};

/// The Fx word hash: rotate, xor, multiply per 8-byte word.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }
    #[inline]
    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }
    #[inline]
    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }
    #[inline]
    fn write_i32(&mut self, n: i32) {
        self.add(n as u32 as u64);
    }
    #[inline]
    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }
    #[inline]
    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
/// `std::collections::HashMap` over [`FxHasher`].
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
/// `std::collections::HashSet` over [`FxHasher`].
pub type FxHashSet<K> = std::collections::HashSet<K, FxBuildHasher>;
