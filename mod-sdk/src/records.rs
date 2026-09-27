use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{log, world_kv_get, world_kv_set, KV_MAX_VALUE_BYTES};

pub trait KvRecord: Sized {
    const VERSION: u8;
    const OLDEST_VERSION: u8 = Self::VERSION;
    fn encode(&self) -> Vec<u8>;
    fn decode(bytes: &[u8]) -> Option<Self>;
    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        let _ = (from, bytes);
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordError {
    Empty,
    Newer { found: u8, newest: u8 },
    Retired { found: u8, oldest: u8 },
    Upgrade { from: u8 },
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

struct Held<T> {
    value: T,
    bytes: Vec<u8>,
}

impl<T: KvRecord> Held<T> {
    fn new(value: T) -> Self {
        let bytes = versioned(&value);
        Self { value, bytes }
    }

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

fn write_fitting(key: &str, bytes: &[u8]) {
    if bytes.len() > KV_MAX_VALUE_BYTES {
        log(&format!(
            "record {key} is {} bytes, over the {KV_MAX_VALUE_BYTES}-byte value cap; \
             not saved (kept in memory)",
            bytes.len()
        ));
        return;
    }
    world_kv_set(key, bytes.to_vec());
}

pub fn encode_versioned<T: KvRecord>(value: &T) -> Vec<u8> {
    versioned(value)
}

pub fn decode_versioned<T: KvRecord>(bytes: &[u8]) -> Result<T, RecordError> {
    unversioned(bytes)
}

pub fn decode_versioned_or_legacy<T: KvRecord>(
    bytes: &[u8],
    legacy_len: usize,
) -> Result<T, RecordError> {
    if bytes.len() == legacy_len {
        let mut framed = Vec::with_capacity(legacy_len + 1);
        framed.push(0);
        framed.extend_from_slice(bytes);
        return unversioned(&framed);
    }
    unversioned(bytes)
}

pub fn world_kv_load<T: KvRecord>(key: &str) -> Result<Option<T>, RecordError> {
    world_kv_get(key)
        .map(|bytes| unversioned(&bytes))
        .transpose()
}

pub fn world_kv_store<T: KvRecord>(key: &str, value: &T) {
    world_kv_set(key, versioned(value));
}

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

pub struct RecordStore<T> {
    prefix: String,
    held: BTreeMap<u64, Held<T>>,
    missing: BTreeSet<u64>,
    unreadable: BTreeMap<u64, RecordError>,
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

    pub fn key(&self, id: u64) -> String {
        format!("{}{id}", self.prefix)
    }

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

    pub fn peek(&self, id: u64) -> Option<&T> {
        self.held.get(&id).map(|held| &held.value)
    }

    pub fn update<R>(&mut self, id: u64, f: impl FnOnce(&mut T) -> R) -> Option<(R, bool)> {
        self.fetch(id);
        let held = self.held.get_mut(&id)?;
        self.touched.insert(id);
        let (out, changed) = held.edit(f);
        if changed {
            write_fitting(&format!("{}{id}", self.prefix), &held.bytes);
        }
        Some((out, changed))
    }

    pub fn insert(&mut self, id: u64, value: T) {
        let held = Held::new(value);
        write_fitting(&self.key(id), &held.bytes);
        self.missing.remove(&id);
        self.unreadable.remove(&id);
        self.touched.insert(id);
        self.held.insert(id, held);
    }

    pub fn loaded(&self) -> impl Iterator<Item = (u64, &T)> {
        self.held.iter().map(|(id, held)| (*id, &held.value))
    }

    pub fn sweep(&mut self, keep: impl Fn(u64, &T) -> bool) {
        let touched = std::mem::take(&mut self.touched);
        self.held
            .retain(|id, held| touched.contains(id) || keep(*id, &held.value));
    }
}

#[cfg(test)]
mod tests;
