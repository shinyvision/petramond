use std::hash::Hash;

use crate::fx::{HashMap, HashSet};

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
    pub fn set(&mut self, key: K, until: u64) {
        self.until.insert(key, until);
    }

    pub fn holds(&self, key: &K, now: u64) -> bool {
        self.until.get(key).is_some_and(|&t| t > now)
    }

    pub fn until(&self, key: &K) -> Option<u64> {
        self.until.get(key).copied()
    }

    pub fn lift(&mut self, key: &K) {
        self.until.remove(key);
    }

    pub fn len(&self) -> usize {
        self.until.len()
    }
}

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
    pub fn strike(&mut self, key: K) {
        self.struck.insert(key);
    }

    pub fn struck(&self, key: &K) -> bool {
        self.struck.contains(key)
    }

    pub fn retain(&mut self, keep: impl FnMut(&K) -> bool) {
        self.struck.retain(keep);
    }
}

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
    pub fn count(&mut self, key: K) -> u8 {
        let (tries, _) = self.tries.entry(key).or_default();
        *tries = tries.saturating_add(1);
        *tries
    }

    pub fn within(&mut self, key: K, most: u8) -> bool {
        self.count(key) <= most
    }

    pub fn count_spaced(&mut self, key: K, now: u64, spacing: u64) -> u8 {
        let (tries, at) = self.tries.entry(key).or_default();
        if *tries == 0 || now >= *at + spacing {
            *tries = tries.saturating_add(1);
            *at = now;
        }
        *tries
    }

    pub fn forget(&mut self, key: &K) {
        self.tries.remove(key);
    }
}
