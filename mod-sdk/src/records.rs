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
//! record's own encoding. A value of an older version is lifted through
//! [`KvRecord::upgrade`] one version at a time, then decoded; it is written
//! back in the current version by its next [`RecordStore::update`]. A value
//! that cannot be read — newer than this build, older than its upgrades
//! reach, or malformed — is a [`RecordError`], never "absent": it is logged
//! once, the store never writes over it through [`RecordStore::update`], and
//! the world keeps its bytes for a build that can read them.
//!
//! The same versioning serves any single persisted value that is not one of
//! many records — a machine's cell-KV state, one world-KV row, a client
//! storage blob: [`encode_versioned`] / [`decode_versioned`] frame a
//! [`KvRecord`] exactly as the store does, and [`world_kv_load`] /
//! [`world_kv_store`] do it for one world-KV key.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{log, world_kv_get, world_kv_set};

/// One kind of persistent record. Pick any encoding; a `serde` type can use
/// the SDK's own wire format: `crate::encode(self).unwrap_or_default()` and
/// `crate::decode(bytes).ok()`.
///
/// Changing the encoding's shape: bump [`VERSION`](Self::VERSION) and teach
/// [`upgrade`](Self::upgrade) to rewrite the previous version's bytes, so
/// stored records migrate instead of becoming unreadable.
pub trait KvRecord: Sized {
    /// The version this build writes.
    const VERSION: u8;
    /// The oldest stored version [`upgrade`](Self::upgrade) can lift. The
    /// default reads only [`VERSION`](Self::VERSION).
    const OLDEST_VERSION: u8 = Self::VERSION;
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Option<Self>;
    /// Rewrite a value stored at version `from` (`OLDEST_VERSION <= from <
    /// VERSION`) as version `from + 1`; `None` when those bytes are
    /// malformed. Called once per step, oldest first.
    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        let _ = (from, bytes);
        None
    }
}

/// Why a stored record could not be read. Never means "absent".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    /// An empty value: not even a version byte.
    Empty,
    /// Written by a newer build of the mod.
    Newer { found: u8, newest: u8 },
    /// Older than the oldest version [`KvRecord::upgrade`] lifts.
    Retired { found: u8, oldest: u8 },
    /// The upgrade step from `from` rejected the bytes.
    Upgrade { from: u8 },
    /// The current-version bytes did not decode.
    Corrupt,
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty value"),
            Self::Newer { found, newest } => write!(
                f,
                "version {found} is newer than this build reads (up to {newest})"
            ),
            Self::Retired { found, oldest } => write!(
                f,
                "version {found} is older than this build migrates (from {oldest})"
            ),
            Self::Upgrade { from } => write!(f, "upgrading from version {from} failed"),
            Self::Corrupt => write!(f, "does not decode"),
        }
    }
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

/// `value` as stored: its version byte, then its own encoding.
pub fn encode_versioned<T: KvRecord>(value: &T) -> Vec<u8> {
    versioned(value)
}

/// Decode a value stored by [`encode_versioned`], lifting an older version
/// through [`KvRecord::upgrade`] first. An error is never "absent": the
/// caller decides whether to keep the bytes or reset.
pub fn decode_versioned<T: KvRecord>(bytes: &[u8]) -> Result<T, RecordError> {
    unversioned(bytes)
}

/// Read one versioned world-KV value: `Ok(None)` when the key is absent, an
/// error when it holds bytes this build cannot read.
pub fn world_kv_load<T: KvRecord>(key: &str) -> Result<Option<T>, RecordError> {
    world_kv_get(key)
        .map(|bytes| unversioned(&bytes))
        .transpose()
}

/// Write one versioned world-KV value.
pub fn world_kv_store<T: KvRecord>(key: &str, value: &T) {
    world_kv_set(key, versioned(value));
}

/// Decode a stored value, lifting an older version through the upgrade
/// chain first.
fn unversioned<T: KvRecord>(bytes: &[u8]) -> Result<T, RecordError> {
    let (&found, body) = bytes.split_first().ok_or(RecordError::Empty)?;
    if found > T::VERSION {
        return Err(RecordError::Newer {
            found,
            newest: T::VERSION,
        });
    }
    if found < T::OLDEST_VERSION {
        return Err(RecordError::Retired {
            found,
            oldest: T::OLDEST_VERSION,
        });
    }
    let mut body = body.to_vec();
    for from in found..T::VERSION {
        body = T::upgrade(from, &body).ok_or(RecordError::Upgrade { from })?;
    }
    T::decode(&body).ok_or(RecordError::Corrupt)
}

/// Every record of one kind this session has read, over world KV keys
/// `"{prefix}{id}"`. The prefix must sit in the mod's own namespace
/// (`"my_mod:thing/"`).
pub struct RecordStore<T> {
    prefix: String,
    held: BTreeMap<u64, Held<T>>,
    /// Ids the world has no record for.
    missing: BTreeSet<u64>,
    /// Ids whose record exists but cannot be read.
    unreadable: BTreeMap<u64, RecordError>,
    /// Ids read or edited since the last [`RecordStore::sweep`].
    touched: BTreeSet<u64>,
}

impl<T: KvRecord> RecordStore<T> {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            held: BTreeMap::new(),
            missing: BTreeSet::new(),
            unreadable: BTreeMap::new(),
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
        if self.held.contains_key(&id)
            || self.missing.contains(&id)
            || self.unreadable.contains_key(&id)
        {
            return;
        }
        let key = self.key(id);
        let Some(bytes) = world_kv_get(&key) else {
            self.missing.insert(id);
            return;
        };
        match unversioned::<T>(&bytes) {
            Ok(value) => {
                self.held.insert(id, Held { value, bytes });
            }
            Err(error) => {
                log(&format!(
                    "record {key} is unreadable ({error}); keeping it untouched"
                ));
                self.unreadable.insert(id, error);
            }
        }
    }

    /// The record, read from the world the first time it is asked for:
    /// `Ok(None)` when the world has none, an error when it has one this
    /// build cannot read.
    pub fn get(&mut self, id: u64) -> Result<Option<&T>, RecordError> {
        self.fetch(id);
        if let Some(error) = self.unreadable.get(&id) {
            return Err(error.clone());
        }
        let Some(held) = self.held.get(&id) else {
            return Ok(None);
        };
        self.touched.insert(id);
        Ok(Some(&held.value))
    }

    /// The record if this session already holds it; never reads the world.
    pub fn peek(&self, id: u64) -> Option<&T> {
        self.held.get(&id).map(|held| &held.value)
    }

    /// Edit the record in place. The world is written only when the edit
    /// changed the record's encoding; the flag says whether it did. `None`
    /// when there is no readable record — an unreadable one is never
    /// written over.
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

    /// Store a new record (or replace one) and write it to the world. This
    /// replaces whatever the world holds under the key, readable or not.
    pub fn insert(&mut self, id: u64, value: T) {
        let held = Held::new(value);
        world_kv_set(&self.key(id), held.bytes.clone());
        self.missing.remove(&id);
        self.unreadable.remove(&id);
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
