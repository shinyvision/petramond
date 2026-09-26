//! Per-thread front tables: small direct-mapped copies of the hottest shared
//! facts, so a worker revisiting what it just read skips the shared memo's
//! lock. Each is declared once as a [`LocalSpec`] so the cache report can
//! bill it (entries × threads holding one) and [`clear_all`] can empty every
//! thread's copy when a world closes.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::MemoStats;

/// Bumped by [`clear_all`]; a table whose stamp is older empties itself on
/// its thread's next lookup.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Lookups a thread counts locally before publishing them to its spec.
const FLUSH_EVERY: u32 = 1024;

/// One per-thread table's declaration and the live totals across threads.
pub(crate) struct LocalSpec {
    name: &'static str,
    entries: usize,
    entry_bytes: AtomicUsize,
    threads: AtomicUsize,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl LocalSpec {
    const fn new(name: &'static str, entries: usize) -> Self {
        assert!(entries.is_power_of_two());
        Self {
            name,
            entries,
            entry_bytes: AtomicUsize::new(0),
            threads: AtomicUsize::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    fn stats(&self) -> MemoStats {
        let threads = self.threads.load(Ordering::Relaxed);
        MemoStats {
            name: self.name,
            shared: false,
            capacity: self.entries * threads,
            entries: 0,
            bytes: (self.entries * self.entry_bytes.load(Ordering::Relaxed) * threads) as u64,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }
}

/// Cave source samples in front of the shared sample memo.
pub(crate) static CAVE_SOURCE: LocalSpec = LocalSpec::new("cave.source.local", 16_384);
/// Cave climate columns in front of the shared column memo.
pub(crate) static CAVE_CLIMATE: LocalSpec = LocalSpec::new("cave.climate.local", 2048);
/// Habitat region tiles in front of the shared tile memo.
pub(crate) static CAVE_REGIONS: LocalSpec = LocalSpec::new("cave.regions.local", 256);
/// Habitat territory grids in front of the shared grid memo.
pub(crate) static CAVE_TERRITORY: LocalSpec = LocalSpec::new("cave.territory.local", 256);
/// Point-query cave lattices (8³ tiles).
pub(crate) static CAVE_POINTS: LocalSpec = LocalSpec::new("cave.points.local", 128);
/// Surface climate quart-cell samples and their base classification.
pub(crate) static SURFACE_CLIMATE: LocalSpec = LocalSpec::new("surface.climate.local", 32_768);
/// The climate domain warp per quart cell.
pub(crate) static CLIMATE_WARP: LocalSpec = LocalSpec::new("surface.warp.local", 1024);

/// Every per-thread table, for the cache report.
static ALL: [&LocalSpec; 7] = [
    &CAVE_SOURCE,
    &CAVE_CLIMATE,
    &CAVE_REGIONS,
    &CAVE_TERRITORY,
    &CAVE_POINTS,
    &SURFACE_CLIMATE,
    &CLIMATE_WARP,
];

pub(crate) fn stats() -> impl Iterator<Item = MemoStats> {
    ALL.iter().map(|spec| spec.stats())
}

/// Empty every thread's copy of every table (lazily, on its next lookup).
pub(crate) fn clear_all() {
    EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// One thread's direct-mapped table for `spec`: `thread_local!` holds it.
/// Values must be pure functions of their key, as in the shared memos: a
/// slot collision only evicts.
pub(crate) struct LocalTable<K, V> {
    spec: &'static LocalSpec,
    slots: RefCell<Box<[Option<(K, V)>]>>,
    epoch: Cell<u64>,
    pending: Cell<(u32, u32)>,
}

impl<K: PartialEq, V: Clone> LocalTable<K, V> {
    pub(crate) fn new(spec: &'static LocalSpec) -> Self {
        spec.threads.fetch_add(1, Ordering::Relaxed);
        spec.entry_bytes
            .store(std::mem::size_of::<Option<(K, V)>>(), Ordering::Relaxed);
        Self {
            spec,
            slots: RefCell::new((0..spec.entries).map(|_| None).collect()),
            epoch: Cell::new(EPOCH.load(Ordering::Relaxed)),
            pending: Cell::new((0, 0)),
        }
    }

    /// The value under `key`, computing and keeping it on a miss. `hash`
    /// picks the slot (its high bits); `compute` runs with no borrow of the
    /// table held, so it may consult this table for other keys.
    pub(crate) fn get_or_insert_with(&self, hash: u64, key: K, compute: impl FnOnce() -> V) -> V {
        let slot = self.slot(hash);
        if let Some(value) = self.lookup(slot, &key) {
            return value;
        }
        let value = compute();
        self.slots.borrow_mut()[slot] = Some((key, value.clone()));
        value
    }

    /// The value under `key`, if this thread holds it.
    pub(crate) fn get(&self, hash: u64, key: &K) -> Option<V> {
        self.lookup(self.slot(hash), key)
    }

    /// Change the value under `key` in place, if this thread holds it.
    pub(crate) fn update(&self, hash: u64, key: &K, change: impl FnOnce(&mut V)) {
        let slot = self.slot(hash);
        if let Some((saved, value)) = &mut self.slots.borrow_mut()[slot] {
            if saved == key {
                change(value);
            }
        }
    }

    /// The slot `hash` selects, after emptying a table a clear has outdated.
    fn slot(&self, hash: u64) -> usize {
        let epoch = EPOCH.load(Ordering::Relaxed);
        if self.epoch.get() != epoch {
            self.slots.borrow_mut().iter_mut().for_each(|s| *s = None);
            self.epoch.set(epoch);
        }
        (hash >> (64 - self.spec.entries.trailing_zeros())) as usize
    }

    fn lookup(&self, slot: usize, key: &K) -> Option<V> {
        let hit = match &self.slots.borrow()[slot] {
            Some((saved, value)) if saved == key => Some(value.clone()),
            _ => None,
        };
        self.count(hit.is_some());
        hit
    }

    fn count(&self, hit: bool) {
        let (mut hits, mut misses) = self.pending.get();
        if hit {
            hits += 1;
        } else {
            misses += 1;
        }
        if hits + misses >= FLUSH_EVERY {
            self.flush(hits, misses);
            (hits, misses) = (0, 0);
        }
        self.pending.set((hits, misses));
    }

    fn flush(&self, hits: u32, misses: u32) {
        self.spec.hits.fetch_add(hits.into(), Ordering::Relaxed);
        self.spec.misses.fetch_add(misses.into(), Ordering::Relaxed);
    }
}

impl<K, V> Drop for LocalTable<K, V> {
    fn drop(&mut self) {
        let (hits, misses) = self.pending.get();
        self.spec.hits.fetch_add(hits.into(), Ordering::Relaxed);
        self.spec.misses.fetch_add(misses.into(), Ordering::Relaxed);
        self.spec.threads.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The usual slot hash for a small integer key: the golden-ratio multiply.
#[inline]
pub(crate) fn spread(bits: u64) -> u64 {
    bits.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST: LocalSpec = LocalSpec::new("test.local", 4);

    #[test]
    fn a_local_table_memoizes_is_billed_and_clears_lazily() {
        let table: LocalTable<u32, u64> = LocalTable::new(&TEST);
        let calls = Cell::new(0);
        let get = |key: u32| {
            table.get_or_insert_with(spread(key.into()), key, || {
                calls.set(calls.get() + 1);
                u64::from(key) * 10
            })
        };
        assert_eq!(get(7), 70);
        assert_eq!(get(7), 70);
        assert_eq!(calls.get(), 1);
        let stats = TEST.stats();
        assert!(!stats.shared);
        assert_eq!(stats.capacity, 4);
        assert!(stats.bytes >= 4 * std::mem::size_of::<u64>() as u64);
        clear_all();
        assert_eq!(get(7), 70);
        assert_eq!(calls.get(), 2, "a clear empties the thread's copy");
        drop(table);
        assert_eq!(TEST.stats().capacity, 0);
        assert_eq!(TEST.stats().hits + TEST.stats().misses, 3);
    }
}
