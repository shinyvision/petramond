use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, Mutex, PoisonError, RwLock};
use std::time::Duration;

/// Entries per set. A direct-mapped slot per key evicts on every hash
/// collision, and a generation frontier's working set sits near a memo's
/// capacity; a few ways per set keep neighbours from evicting each other.
const WAYS: usize = 4;

/// A value some worker is deriving right now; the rest wait on it instead of
/// deriving it again or waiting behind the whole set.
struct Flight {
    done: Mutex<bool>,
    ready: Condvar,
    started: std::time::Instant,
}

impl Flight {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            done: Mutex::new(false),
            ready: Condvar::new(),
            started: std::time::Instant::now(),
        })
    }

    /// Wait for the derivation; `false` once it looks abandoned, so the
    /// waiter derives the value itself rather than hanging on a lost worker.
    fn wait(&self) -> bool {
        const ABANDONED: Duration = Duration::from_secs(10);
        let done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        let (done, _) = self
            .ready
            .wait_timeout_while(done, ABANDONED, |done| !*done)
            .unwrap_or_else(PoisonError::into_inner);
        *done || self.started.elapsed() < ABANDONED
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
}

type Slot<K, V> = RwLock<Set<K, V>>;

/// Bounded storage for expensive positional facts shared by generation workers.
/// Each set has its own lock, and readers never exclude each other: unrelated
/// samples can be computed concurrently, and hits from many workers on the
/// same set proceed in parallel.
pub(crate) struct SharedMemo<K, V> {
    slots: Box<[Slot<K, V>]>,
}

impl<K: Eq + Hash + Clone, V: Clone> SharedMemo<K, V> {
    /// `capacity` counts entries; it must be a power of two of at least one set.
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two() && capacity >= WAYS);
        Self {
            slots: (0..capacity / WAYS)
                .map(|_| {
                    RwLock::new(Set {
                        entries: std::array::from_fn(|_| None),
                        next: 0,
                    })
                })
                .collect(),
        }
    }

    fn slot<Q: Hash + ?Sized>(&self, key: &Q) -> &Slot<K, V> {
        let mut hash = rustc_hash::FxHasher::default();
        key.hash(&mut hash);
        &self.slots[hash.finish() as usize & (self.slots.len() - 1)]
    }

    /// The memoized value, if any, without computing one.
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        let guard = self
            .slot(key)
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        match guard.find(key) {
            Some(Entry::Ready(value)) => Some(value.clone()),
            _ => None,
        }
    }

    /// `compute` must be a pure function of the complete key and must not
    /// recursively access this memo for the same key. One worker derives a
    /// missing value while the others asking for that key wait for it; other
    /// keys of the set are never held up.
    pub(crate) fn get_or_insert(&self, key: K, compute: impl FnOnce() -> V) -> V {
        let slot = self.slot(&key);
        if let Some(value) = self.get(&key) {
            return value;
        }
        let mut compute = Some(compute);
        loop {
            let flight = {
                let mut guard = slot.write().unwrap_or_else(PoisonError::into_inner);
                match guard.find(&key) {
                    Some(Entry::Ready(value)) => return value.clone(),
                    Some(Entry::Computing(flight)) => Arc::clone(flight),
                    None => {
                        let flight = Flight::new();
                        guard.store(key.clone(), Entry::Computing(Arc::clone(&flight)));
                        drop(guard);
                        let value = compute.take().expect("one derivation per call")();
                        slot.write()
                            .unwrap_or_else(PoisonError::into_inner)
                            .store(key, Entry::Ready(value.clone()));
                        flight.finish();
                        return value;
                    }
                }
            };
            if !flight.wait() {
                // Abandoned: derive it here, publishing over the stale marker.
                let value = compute.take().expect("one derivation per call")();
                slot.write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .store(key, Entry::Ready(value.clone()));
                flight.finish();
                return value;
            }
        }
    }

    /// [`get_or_insert`](Self::get_or_insert) for a cheap value: `compute`
    /// runs without any coordination, so concurrent misses may compute the
    /// same pure value twice. Right for per-corner samples; wrong for
    /// anything whose one-time cost is worth waiting for.
    pub(crate) fn get_or_compute_unlocked(&self, key: K, compute: impl FnOnce() -> V) -> V {
        if let Some(value) = self.get(&key) {
            return value;
        }
        let value = compute();
        self.slot(&key)
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .store(key, Entry::Ready(value.clone()));
        value
    }

    /// A hit for a request that is only borrowed: `key` must hash exactly as
    /// the owned key it stands for, and `matches` compares the complete key.
    pub(crate) fn find<Q: Hash + ?Sized>(
        &self,
        key: &Q,
        matches: impl Fn(&K) -> bool,
    ) -> Option<V> {
        let guard = self
            .slot(key)
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        guard
            .entries
            .iter()
            .flatten()
            .find_map(|(saved, entry)| match entry {
                Entry::Ready(value) if matches(saved) => Some(value.clone()),
                _ => None,
            })
    }

    /// Publish a completed value.
    pub(crate) fn insert(&self, key: K, value: V) {
        self.slot(&key)
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .store(key, Entry::Ready(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn borrowed_requests_reach_the_owned_keys_slot() {
        let memo = SharedMemo::new(16);
        let request = [(12, -5), (3, 8)];
        memo.get_or_insert(Box::<[_]>::from(request), || 42);
        let hit = memo.find(request.as_slice(), |saved| saved.as_ref() == request);
        assert_eq!(hit, Some(42));
        assert_eq!(memo.find(request.as_slice(), |_| false), None);
    }
}
