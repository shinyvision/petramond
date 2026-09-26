//! Work set aside between ticks: until a tick, for good, or after so many
//! tries. Every patience the crew keeps is one of these three, so how long a
//! thing waits is read the same way wherever it is kept.

use std::hash::Hash;

use crate::fx::{HashMap, HashSet};

/// Keys set aside until a tick: held while the tick lies ahead.
pub struct Until<K> {
    until: HashMap<K, u64>,
}

impl<K> Default for Until<K> {
    fn default() -> Self {
        Self {
            until: HashMap::default(),
        }
    }
}

impl<K: Eq + Hash> Until<K> {
    /// Set `key` aside until `until`, replacing any earlier wait.
    pub fn set(&mut self, key: K, until: u64) {
        self.until.insert(key, until);
    }

    /// Whether `key` still waits at `now`.
    pub fn holds(&self, key: &K, now: u64) -> bool {
        self.until.get(key).is_some_and(|&t| t > now)
    }

    /// The tick `key` waits until, held or lapsed.
    pub fn until(&self, key: &K) -> Option<u64> {
        self.until.get(key).copied()
    }

    /// Wake `key` now.
    pub fn lift(&mut self, key: &K) {
        self.until.remove(key);
    }

    /// Keys ever set aside and not lifted, lapsed or not.
    pub fn len(&self) -> usize {
        self.until.len()
    }
}

/// Keys ruled out for good, until something forgives them.
pub struct Struck<K> {
    struck: HashSet<K>,
}

impl<K> Default for Struck<K> {
    fn default() -> Self {
        Self {
            struck: HashSet::default(),
        }
    }
}

impl<K: Eq + Hash> Struck<K> {
    /// Never `key` again.
    pub fn strike(&mut self, key: K) {
        self.struck.insert(key);
    }

    /// Whether `key` was ruled out.
    pub fn struck(&self, key: &K) -> bool {
        self.struck.contains(key)
    }

    /// Forgive every key `keep` refuses.
    pub fn retain(&mut self, keep: impl FnMut(&K) -> bool) {
        self.struck.retain(keep);
    }
}

/// Tries counted per key, and the tick each was last counted at.
pub struct Tries<K> {
    tries: HashMap<K, (u8, u64)>,
}

impl<K> Default for Tries<K> {
    fn default() -> Self {
        Self {
            tries: HashMap::default(),
        }
    }
}

impl<K: Eq + Hash> Tries<K> {
    /// Count another try of `key`; how many it has had.
    pub fn count(&mut self, key: K) -> u8 {
        let (tries, _) = self.tries.entry(key).or_default();
        *tries = tries.saturating_add(1);
        *tries
    }

    /// Count another round `key` has waited; whether it still waits, having
    /// waited no more than `most`.
    pub fn within(&mut self, key: K, most: u8) -> bool {
        self.count(key) <= most
    }

    /// Count a try of `key` at `now`, where tries closer together than
    /// `spacing` are one; how many it has had.
    pub fn count_spaced(&mut self, key: K, now: u64, spacing: u64) -> u8 {
        let (tries, at) = self.tries.entry(key).or_default();
        if *tries == 0 || now >= *at + spacing {
            *tries = tries.saturating_add(1);
            *at = now;
        }
        *tries
    }

    /// Start `key` over.
    pub fn forget(&mut self, key: &K) {
        self.tries.remove(key);
    }
}
