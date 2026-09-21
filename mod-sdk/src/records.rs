//! Typed, versioned records in world KV, one per `u64` id.
//!
//! A mod that keeps persistent things of its own (projects, contracts,
//! claims) keeps each as one world-KV value. [`RecordStore`] is the session's
//! view of them: a record is read and decoded once, edits go through
//! [`RecordStore::update`], and the world is written ONLY when an edit
//! changed the record's bytes — so "update every tick, change rarely" costs
//! one encode and no host call.
//!
//! The stored value is one version byte ([`KvRecord::VERSION`]) and then the
//! record's own encoding. A value of another version, or one that does not
//! decode, is reported to the log once and from then on reads as absent
//! without asking the world again.

use std::collections::{BTreeMap, BTreeSet};

use crate::{log, world_kv_get, world_kv_set};

/// One kind of persistent record. Pick any encoding; a `serde` type can use
/// the SDK's own wire format: `crate::encode(self).unwrap_or_default()` and
/// `crate::decode(bytes).ok()`.
pub trait KvRecord: Sized {
    /// Bump when the encoding changes shape: older values then read as
    /// absent instead of decoding into nonsense.
    const VERSION: u8;
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Option<Self>;
}

/// A decoded record beside the bytes the world holds for it.
struct Held<T> {
    value: T,
    bytes: Vec<u8>,
}

impl<T: KvRecord> Held<T> {
    fn new(value: T) -> Self {
        let bytes = versioned(&value);
        Self { value, bytes }
    }

    /// Run `f`; `true` when the record's bytes changed (and were taken over).
    fn edit<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> (R, bool) {
        let out = f(&mut self.value);
        let bytes = versioned(&self.value);
        let changed = bytes != self.bytes;
        if changed {
            self.bytes = bytes;
        }
        (out, changed)
    }
}

fn versioned<T: KvRecord>(value: &T) -> Vec<u8> {
    let mut bytes = vec![T::VERSION];
    bytes.extend(value.encode());
    bytes
}

fn unversioned<T: KvRecord>(bytes: &[u8]) -> Option<T> {
    match bytes.split_first() {
        Some((version, rest)) if *version == T::VERSION => T::decode(rest),
        _ => None,
    }
}

/// Every record of one kind this session has read, over world KV keys
/// `"{prefix}{id}"`. The prefix must sit in the mod's own namespace
/// (`"my_mod:thing/"`).
pub struct RecordStore<T> {
    prefix: String,
    held: BTreeMap<u64, Held<T>>,
    /// Ids the world has no readable record for.
    missing: BTreeSet<u64>,
    /// Ids read or edited since the last [`RecordStore::sweep`].
    touched: BTreeSet<u64>,
}

impl<T: KvRecord> RecordStore<T> {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            held: BTreeMap::new(),
            missing: BTreeSet::new(),
            touched: BTreeSet::new(),
        }
    }

    /// The world KV key of record `id`.
    pub fn key(&self, id: u64) -> String {
        format!("{}{id}", self.prefix)
    }

    /// The id a key of this store names.
    pub fn id_of_key(&self, key: &str) -> Option<u64> {
        key.strip_prefix(&self.prefix)?.parse().ok()
    }

    fn fetch(&mut self, id: u64) {
        if self.held.contains_key(&id) || self.missing.contains(&id) {
            return;
        }
        let key = self.key(id);
        let held = world_kv_get(&key).and_then(|bytes| {
            let value = unversioned::<T>(&bytes);
            if value.is_none() {
                log(&format!(
                    "record {key} does not decode as version {}; ignoring it",
                    T::VERSION
                ));
            }
            Some(Held {
                value: value?,
                bytes,
            })
        });
        if let Some(held) = held {
            self.held.insert(id, held);
            return;
        }
        self.missing.insert(id);
    }

    /// The record, read from the world the first time it is asked for.
    pub fn get(&mut self, id: u64) -> Option<&T> {
        self.fetch(id);
        let held = self.held.get(&id)?;
        self.touched.insert(id);
        Some(&held.value)
    }

    /// The record if this session already holds it; never reads the world.
    pub fn peek(&self, id: u64) -> Option<&T> {
        self.held.get(&id).map(|held| &held.value)
    }

    /// Edit the record in place. The world is written only when the edit
    /// changed the record's encoding; the flag says whether it did.
    pub fn update<R>(&mut self, id: u64, f: impl FnOnce(&mut T) -> R) -> Option<(R, bool)> {
        self.fetch(id);
        let held = self.held.get_mut(&id)?;
        self.touched.insert(id);
        let (out, changed) = held.edit(f);
        if changed {
            world_kv_set(&format!("{}{id}", self.prefix), held.bytes.clone());
        }
        Some((out, changed))
    }

    /// Store a new record (or replace one) and write it to the world.
    pub fn insert(&mut self, id: u64, value: T) {
        let held = Held::new(value);
        world_kv_set(&self.key(id), held.bytes.clone());
        self.missing.remove(&id);
        self.touched.insert(id);
        self.held.insert(id, held);
    }

    /// Records in memory right now, by id. Says nothing about records the
    /// session has not asked for.
    pub fn loaded(&self) -> impl Iterator<Item = (u64, &T)> {
        self.held.iter().map(|(id, held)| (*id, &held.value))
    }

    /// Drop from memory every record neither read nor edited since the last
    /// sweep, unless `keep` wants it. The world keeps them all: a dropped
    /// record is read again when next asked for. Call it on a slow cadence to
    /// bound a long session's memory.
    pub fn sweep(&mut self, keep: impl Fn(u64, &T) -> bool) {
        let touched = std::mem::take(&mut self.touched);
        self.held
            .retain(|id, held| touched.contains(id) || keep(*id, &held.value));
    }
}

#[cfg(test)]
mod tests;
