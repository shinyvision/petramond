//! The shared memo: a bounded, set-associative table of positional facts that
//! every generation worker reads and fills, with single-flight derivation and
//! the counters the cache report is built from.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, RwLock};

use super::{CacheBudget, MemoStats, Scaling};

/// Entries per set. A direct-mapped slot per key evicts on every hash
/// collision, and a generation frontier's working set sits near a memo's
/// capacity; a few ways per set keep neighbours from evicting each other.
const WAYS: usize = 4;

/// A value some worker is deriving right now; the rest wait on it instead of
/// deriving it again or waiting behind the whole set.
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

    /// Wait until the deriving worker publishes its value or unwinds.
    fn wait(&self) {
        let done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .ready
            .wait_while(done, |done| !*done)
            .unwrap_or_else(PoisonError::into_inner);
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
    /// Round-robin victim once every way is taken.
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

    /// Drop `key`'s marker if it is still the in-flight `flight`.
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

/// One set and its lookup counters. The counters share the set's cache line,
/// which every lookup already writes through the lock, so counting adds no
/// contention of its own.
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

/// The one worker deriving a key. It either publishes the value or, when the
/// deriving closure unwinds (or the derivation is otherwise dropped
/// unpublished), withdraws the in-flight marker and wakes every waiter at
/// once, so they take the derivation over (meeting the same panic in their
/// own thread if the input is bad) instead of waiting on a value that will
/// never come.
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

/// Bounded storage for expensive positional facts shared by generation workers.
/// Each set has its own lock, and readers never exclude each other: unrelated
/// samples can be computed concurrently, and hits from many workers on the
/// same set proceed in parallel.
struct SharedMemo<K, V> {
    slots: Box<[Slot<K, V>]>,
}

impl<K: Eq + Hash + Clone, V: Clone> SharedMemo<K, V> {
    /// `capacity` counts entries; it must be a power of two of at least one set.
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

    /// The memoized value, if any, without computing one.
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        let slot = self.slot(key);
        let value = Self::ready(slot, key);
        slot.count(value.is_some());
        value
    }

    /// `compute` must be a pure function of the complete key and must not
    /// recursively access this memo for the same key. One worker derives a
    /// missing value while the others asking for that key wait for it; other
    /// keys of the set are never held up. A derivation that unwinds hands the
    /// key to the next waiter.
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

    /// [`get_or_insert`](Self::get_or_insert) for a cheap value: `compute`
    /// runs without any coordination, so concurrent misses may compute the
    /// same pure value twice. Right for per-corner samples; wrong for
    /// anything whose one-time cost is worth waiting for.
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

    /// A hit for a request that is only borrowed: `key` must hash exactly as
    /// the owned key it stands for, and `matches` compares the complete key.
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

    /// Publish a completed value.
    pub(crate) fn insert(&self, key: K, value: V) {
        self.slot(&key).write().store(key, Entry::Ready(value));
    }

    /// Drop every published value. A derivation in flight still publishes
    /// its own when it completes.
    fn clear(&self) {
        for slot in self.slots.iter() {
            for entry in slot.write().entries.iter_mut() {
                if matches!(entry, Some((_, Entry::Ready(_)))) {
                    *entry = None;
                }
            }
        }
    }

    /// `(entries, heap bytes the values hold, hits, misses)`.
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

    /// Bytes of the table itself: every way of every set, filled or not.
    fn table_bytes(&self) -> usize {
        self.slots.len() * std::mem::size_of::<Slot<K, V>>()
    }
}

/// How a [`Memo`] is declared: its report name, the capacity it was tuned at
/// (the [`CacheBudget::REFERENCE`] world), how that scales, and how to bill a
/// value's heap beyond its inline bytes.
pub(crate) struct MemoSpec<V> {
    pub name: &'static str,
    pub capacity: usize,
    pub scaling: Scaling,
    pub weigh: fn(&V) -> usize,
}

/// A named [`SharedMemo`] sized by a [`CacheBudget`]. The table is allocated
/// on first use, so a world that never generates (a client replica) or a
/// memo a world never reaches costs nothing.
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

    /// See [`SharedMemo::get`].
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        self.table().get(key)
    }

    /// See [`SharedMemo::get_or_insert`].
    pub(crate) fn get_or_insert(&self, key: K, compute: impl FnOnce() -> V) -> V {
        self.table().get_or_insert(key, compute)
    }

    /// See [`SharedMemo::get_or_compute_unlocked`].
    pub(crate) fn get_or_compute_unlocked(&self, key: K, compute: impl FnOnce() -> V) -> V {
        self.table().get_or_compute_unlocked(key, compute)
    }

    /// See [`SharedMemo::find`].
    pub(crate) fn find<Q: Hash + ?Sized>(
        &self,
        key: &Q,
        matches: impl Fn(&K) -> bool,
    ) -> Option<V> {
        self.table().find(key, matches)
    }

    /// See [`SharedMemo::insert`].
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
        // A full set evicts one way and never answers with another key's value.
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
            // Another key of the same set answers while key 1 is being derived.
            assert_eq!(memo.get_or_insert(2, || "fast"), "fast");
            assert_eq!(memo.get(&1), None);
            done.store(true, Ordering::Release);
        });
        assert_eq!(memo.get(&1), Some("slow"));
    }

    /// A derivation that panics must not strand the workers waiting on it:
    /// the next one takes the key over at once, with no timeout involved.
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

    /// A key whose derivation always fails wakes every waiter the moment it
    /// fails; each retries, fails in its own thread and wakes the rest, so
    /// the panic reaches every caller without any of them stalling.
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
        // The key is left free rather than poisoned: a good derivation succeeds.
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

    /// The report sees every lookup and every entry, and a clear empties the
    /// table without forgetting its size.
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
        assert!(stats.bytes >= 1000, "values' heap is billed: {}", stats.bytes);
        memo.clear();
        let cleared = memo.stats();
        assert_eq!(cleared.entries, 0);
        assert_eq!(cleared.capacity, 64);
        assert_eq!(memo.get_or_insert(3, || vec![1].into()).as_ref(), [1]);
    }
}
