use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, RwLock};

use super::{CacheBudget, MemoStats, Scaling};

const WAYS: usize = 4;

struct Flight {
    done: Mutex<bool>,
    ready: Condvar,
}

impl Flight {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Mutex::new(false),
            ready: Condvar::new(),
        })
    }

    fn wait(&self) {
        let done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        drop(
            self.ready
                .wait_while(done, |done| !*done)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    fn finish(&self) {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.ready.notify_all();
    }
}

enum Entry<V> {
    Ready(V),
    Computing(Arc<Flight>),
}

struct Set<K, V> {
    entries: [Option<(K, Entry<V>)>; WAYS],
    next: u8,
}

impl<K: Eq, V> Set<K, V> {
    fn find(&self, key: &K) -> Option<&Entry<V>> {
        self.entries
            .iter()
            .flatten()
            .find(|(saved, _)| saved == key)
            .map(|(_, entry)| entry)
    }

    fn store(&mut self, key: K, entry: Entry<V>) {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|e| e.as_ref().is_some_and(|(saved, _)| *saved == key))
        {
            *slot = Some((key, entry));
            return;
        }
        if let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some((key, entry));
            return;
        }
        let victim = usize::from(self.next) % WAYS;
        self.next = ((victim + 1) % WAYS) as u8;
        self.entries[victim] = Some((key, entry));
    }

    fn abandon(&mut self, key: &K, flight: &Arc<Flight>) {
        for slot in &mut self.entries {
            let stale = matches!(
                slot,
                Some((saved, Entry::Computing(f))) if saved == key && Arc::ptr_eq(f, flight)
            );
            if stale {
                *slot = None;
            }
        }
    }
}

struct Slot<K, V> {
    set: RwLock<Set<K, V>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl<K, V> Slot<K, V> {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Set<K, V>> {
        self.set.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Set<K, V>> {
        self.set.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn count(&self, hit: bool) {
        let counter = if hit { &self.hits } else { &self.misses };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

struct Derivation<'a, K: Eq, V> {
    slot: &'a Slot<K, V>,
    key: &'a K,
    flight: Arc<Flight>,
    published: bool,
}

impl<K: Eq + Clone, V: Clone> Derivation<'_, K, V> {
    fn publish(mut self, value: V) -> V {
        self.slot
            .write()
            .store(self.key.clone(), Entry::Ready(value.clone()));
        self.published = true;
        self.flight.finish();
        value
    }
}

impl<K: Eq, V> Drop for Derivation<'_, K, V> {
    fn drop(&mut self) {
        if !self.published {
            self.slot.write().abandon(self.key, &self.flight);
            self.flight.finish();
        }
    }
}

struct SharedMemo<K, V> {
    slots: Box<[Slot<K, V>]>,
}

impl<K: Eq + Hash + Clone, V: Clone> SharedMemo<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two() && capacity >= WAYS);
        Self {
            slots: (0..capacity / WAYS)
                .map(|_| Slot {
                    set: RwLock::new(Set {
                        entries: std::array::from_fn(|_| None),
                        next: 0,
                    }),
                    hits: AtomicU64::new(0),
                    misses: AtomicU64::new(0),
                })
                .collect(),
        }
    }

    fn slot<Q: Hash + ?Sized>(&self, key: &Q) -> &Slot<K, V> {
        let mut hash = rustc_hash::FxHasher::default();
        key.hash(&mut hash);
        &self.slots[hash.finish() as usize & (self.slots.len() - 1)]
    }

    fn ready(slot: &Slot<K, V>, key: &K) -> Option<V> {
        match slot.read().find(key) {
            Some(Entry::Ready(value)) => Some(value.clone()),
            _ => None,
        }
    }

    pub(crate) fn get(&self, key: &K) -> Option<V> {
        let slot = self.slot(key);
        let value = Self::ready(slot, key);
        slot.count(value.is_some());
        value
    }

    pub(crate) fn contains(&self, key: &K) -> bool {
        Self::ready(self.slot(key), key).is_some()
    }

    pub(crate) fn get_or_insert(&self, key: K, compute: impl FnOnce() -> V) -> V {
        let slot = self.slot(&key);
        if let Some(value) = Self::ready(slot, &key) {
            slot.count(true);
            return value;
        }
        slot.count(false);
        let mut compute = Some(compute);
        loop {
            let flight = {
                let mut set = slot.write();
                match set.find(&key) {
                    Some(Entry::Ready(value)) => return value.clone(),
                    Some(Entry::Computing(flight)) => Arc::clone(flight),
                    None => {
                        let flight = Flight::new();
                        set.store(key.clone(), Entry::Computing(Arc::clone(&flight)));
                        drop(set);
                        let derivation = Derivation {
                            slot,
                            key: &key,
                            flight,
                            published: false,
                        };
                        let value = compute.take().expect("one derivation per call")();
                        return derivation.publish(value);
                    }
                }
            };
            flight.wait();
        }
    }

    pub(crate) fn get_or_compute_unlocked(&self, key: K, compute: impl FnOnce() -> V) -> V {
        let slot = self.slot(&key);
        if let Some(value) = Self::ready(slot, &key) {
            slot.count(true);
            return value;
        }
        slot.count(false);
        let value = compute();
        slot.write().store(key, Entry::Ready(value.clone()));
        value
    }

    pub(crate) fn find<Q: Hash + ?Sized>(
        &self,
        key: &Q,
        matches: impl Fn(&K) -> bool,
    ) -> Option<V> {
        let slot = self.slot(key);
        let value = slot
            .read()
            .entries
            .iter()
            .flatten()
            .find_map(|(saved, entry)| match entry {
                Entry::Ready(value) if matches(saved) => Some(value.clone()),
                _ => None,
            });
        slot.count(value.is_some());
        value
    }

    pub(crate) fn insert(&self, key: K, value: V) {
        self.slot(&key).write().store(key, Entry::Ready(value));
    }

    fn clear(&self) {
        for slot in self.slots.iter() {
            for entry in slot.write().entries.iter_mut() {
                if matches!(entry, Some((_, Entry::Ready(_)))) {
                    *entry = None;
                }
            }
        }
    }

    fn census(&self, weigh: fn(&V) -> usize) -> (usize, usize, u64, u64) {
        let (mut entries, mut heap, mut hits, mut misses) = (0, 0, 0, 0);
        for slot in self.slots.iter() {
            for (_, entry) in slot.read().entries.iter().flatten() {
                if let Entry::Ready(value) = entry {
                    entries += 1;
                    heap += weigh(value);
                }
            }
            hits += slot.hits.load(Ordering::Relaxed);
            misses += slot.misses.load(Ordering::Relaxed);
        }
        (entries, heap, hits, misses)
    }

    fn table_bytes(&self) -> usize {
        self.slots.len() * std::mem::size_of::<Slot<K, V>>()
    }
}

pub(crate) struct MemoSpec<V> {
    pub name: &'static str,
    pub capacity: usize,
    pub scaling: Scaling,
    pub weigh: fn(&V) -> usize,
}

pub(crate) struct Memo<K, V> {
    spec: MemoSpec<V>,
    capacity: usize,
    table: OnceLock<SharedMemo<K, V>>,
}

impl<K: Eq + Hash + Clone, V: Clone> Memo<K, V> {
    pub(crate) fn new(spec: MemoSpec<V>, budget: CacheBudget) -> Self {
        let capacity = budget.capacity(spec.capacity, spec.scaling).max(WAYS);
        Self {
            spec,
            capacity,
            table: OnceLock::new(),
        }
    }

    fn table(&self) -> &SharedMemo<K, V> {
        self.table.get_or_init(|| SharedMemo::new(self.capacity))
    }

    pub(crate) fn get(&self, key: &K) -> Option<V> {
        self.table().get(key)
    }

    pub(crate) fn contains(&self, key: &K) -> bool {
        self.table().contains(key)
    }

    pub(crate) fn get_or_insert(&self, key: K, compute: impl FnOnce() -> V) -> V {
        self.table().get_or_insert(key, compute)
    }

    pub(crate) fn get_or_compute_unlocked(&self, key: K, compute: impl FnOnce() -> V) -> V {
        self.table().get_or_compute_unlocked(key, compute)
    }

    pub(crate) fn find<Q: Hash + ?Sized>(
        &self,
        key: &Q,
        matches: impl Fn(&K) -> bool,
    ) -> Option<V> {
        self.table().find(key, matches)
    }

    pub(crate) fn insert(&self, key: K, value: V) {
        self.table().insert(key, value);
    }

    pub(crate) fn clear(&self) {
        if let Some(table) = self.table.get() {
            table.clear();
        }
    }

    pub(crate) fn stats(&self) -> MemoStats {
        let (entries, heap, hits, misses) = self
            .table
            .get()
            .map_or((0, 0, 0, 0), |table| table.census(self.spec.weigh));
        MemoStats {
            name: self.spec.name,
            shared: true,
            capacity: self.capacity,
            entries,
            bytes: (self.table.get().map_or(0, SharedMemo::table_bytes) + heap) as u64,
            hits,
            misses,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[test]
    fn workers_share_computation_and_collisions_never_alias() {
        let memo = SharedMemo::new(WAYS);
        let calls = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    assert_eq!(
                        memo.get_or_insert((7, -3), || {
                            calls.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(5));
                            42
                        }),
                        42
                    );
                });
            }
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(memo.get_or_insert((8, -3), || 19), 19);
        assert_eq!(memo.get_or_insert((7, -3), || 42), 42);
        assert_eq!(memo.get_or_compute_unlocked((9, 0), || 5), 5);
        assert_eq!(memo.get_or_compute_unlocked((9, 0), || 6), 5);
        for key in 10..20 {
            assert_eq!(memo.get_or_insert((key, 0), || key), key);
        }
        for key in 10..20 {
            assert_eq!(memo.get_or_insert((key, 0), || key), key);
        }
    }

    #[test]
    fn a_derivation_in_flight_blocks_only_its_own_key() {
        let memo = SharedMemo::new(WAYS);
        let (started, done) = (
            std::sync::Barrier::new(2),
            std::sync::atomic::AtomicBool::new(false),
        );
        std::thread::scope(|scope| {
            scope.spawn(|| {
                memo.get_or_insert(1, || {
                    started.wait();
                    while !done.load(Ordering::Acquire) {
                        std::thread::yield_now();
                    }
                    "slow"
                });
            });
            started.wait();
            assert_eq!(memo.get_or_insert(2, || "fast"), "fast");
            assert_eq!(memo.get(&1), None);
            done.store(true, Ordering::Release);
        });
        assert_eq!(memo.get(&1), Some("slow"));
    }

    #[test]
    fn an_unwinding_derivation_hands_the_key_to_a_waiter() {
        let memo = SharedMemo::new(WAYS);
        let started = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let doomed = scope.spawn(|| {
                memo.get_or_insert(3, || {
                    started.wait();
                    std::thread::sleep(Duration::from_millis(20));
                    panic!("derivation failed");
                })
            });
            started.wait();
            assert_eq!(memo.get_or_insert(3, || 9), 9);
            assert!(doomed.join().is_err());
        });
        assert_eq!(memo.get(&3), Some(9));
    }

    #[test]
    fn a_failing_derivation_propagates_to_every_waiter_promptly() {
        let memo: SharedMemo<i32, i32> = SharedMemo::new(WAYS);
        let (started, attempts) = (std::sync::Barrier::new(5), AtomicUsize::new(0));
        let begun = std::time::Instant::now();
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..5)
                .map(|_| {
                    scope.spawn(|| {
                        started.wait();
                        memo.get_or_insert(4, || {
                            attempts.fetch_add(1, Ordering::Relaxed);
                            std::thread::sleep(Duration::from_millis(10));
                            panic!("bad input");
                        })
                    })
                })
                .collect();
            for worker in workers {
                assert!(worker.join().is_err(), "every caller sees the panic");
            }
        });
        assert_eq!(attempts.load(Ordering::Relaxed), 5);
        assert!(
            begun.elapsed() < Duration::from_secs(2),
            "waiters were woken on unwind, not by a timeout: {:?}",
            begun.elapsed()
        );
        assert_eq!(memo.get(&4), None);
        assert_eq!(memo.get_or_insert(4, || 11), 11);
    }

    #[test]
    fn borrowed_requests_reach_the_owned_keys_slot() {
        let memo = SharedMemo::new(16);
        let request = [(12, -5), (3, 8)];
        memo.get_or_insert(Box::<[_]>::from(request), || 42);
        let hit = memo.find(request.as_slice(), |saved| saved.as_ref() == request);
        assert_eq!(hit, Some(42));
        assert_eq!(memo.find(request.as_slice(), |_| false), None);
    }

    #[test]
    fn a_memo_counts_lookups_and_clears() {
        let memo: Memo<u32, Arc<[u8]>> = Memo::new(
            MemoSpec {
                name: "test.bytes",
                capacity: 64,
                scaling: Scaling::Frontier,
                weigh: |v| v.len(),
            },
            CacheBudget::REFERENCE,
        );
        let empty = memo.stats();
        assert_eq!((empty.entries, empty.bytes, empty.hits), (0, 0, 0));
        for key in 0..10u32 {
            memo.get_or_insert(key, || vec![0; 100].into());
        }
        memo.get_or_insert(3, || unreachable!("memoized"));
        assert_eq!(memo.get(&99), None);
        let stats = memo.stats();
        assert_eq!(stats.name, "test.bytes");
        assert_eq!(stats.capacity, 64);
        assert_eq!(stats.entries, 10);
        assert_eq!((stats.hits, stats.misses), (1, 11));
        assert!(
            stats.bytes >= 1000,
            "values' heap is billed: {}",
            stats.bytes
        );
        memo.clear();
        let cleared = memo.stats();
        assert_eq!(cleared.entries, 0);
        assert_eq!(cleared.capacity, 64);
        assert_eq!(memo.get_or_insert(3, || vec![1].into()).as_ref(), [1]);
    }
}
