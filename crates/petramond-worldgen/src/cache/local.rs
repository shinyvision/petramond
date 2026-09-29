use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::MemoStats;

static EPOCH: AtomicU64 = AtomicU64::new(0);

const FLUSH_EVERY: u32 = 1024;

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

pub(crate) static TERRAIN_COLUMNS: LocalSpec = LocalSpec::new("terrain.columns.local", 256);
pub(crate) static CAVE_REGIONS: LocalSpec = LocalSpec::new("cave.regions.local", 256);
pub(crate) static CAVE_TERRITORY: LocalSpec = LocalSpec::new("cave.territory.local", 256);
pub(crate) static CAVE_POINTS: LocalSpec = LocalSpec::new("cave.points.local", 128);
pub(crate) static SURFACE_CLIMATE: LocalSpec = LocalSpec::new("surface.climate.local", 32_768);
pub(crate) static CLIMATE_WARP: LocalSpec = LocalSpec::new("surface.warp.local", 1024);

static ALL: [&LocalSpec; 6] = [
    &TERRAIN_COLUMNS,
    &CAVE_REGIONS,
    &CAVE_TERRITORY,
    &CAVE_POINTS,
    &SURFACE_CLIMATE,
    &CLIMATE_WARP,
];

pub(crate) fn stats() -> impl Iterator<Item = MemoStats> {
    ALL.iter().map(|spec| spec.stats())
}

pub(crate) fn clear_all() {
    EPOCH.fetch_add(1, Ordering::Relaxed);
}

type Slots<K, V> = RefCell<Box<[Option<(K, V)>]>>;

pub(crate) struct LocalTable<K, V> {
    spec: &'static LocalSpec,
    slots: Slots<K, V>,
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

    pub(crate) fn get_or_insert_with(&self, hash: u64, key: K, compute: impl FnOnce() -> V) -> V {
        let slot = self.slot(hash);
        if let Some(value) = self.lookup(slot, &key) {
            return value;
        }
        let value = compute();
        self.slots.borrow_mut()[slot] = Some((key, value.clone()));
        value
    }

    pub(crate) fn get(&self, hash: u64, key: &K) -> Option<V> {
        self.lookup(self.slot(hash), key)
    }

    pub(crate) fn update(&self, hash: u64, key: &K, change: impl FnOnce(&mut V)) {
        let slot = self.slot(hash);
        if let Some((saved, value)) = &mut self.slots.borrow_mut()[slot] {
            if saved == key {
                change(value);
            }
        }
    }

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
