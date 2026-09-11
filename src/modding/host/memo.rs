//! The shared derived-fact memo calls: a process-wide, bounded, discardable
//! store of values that are pure functions of `(mod, world seed, key)`.
//!
//! Generation instances are per thread and share no guest memory, so a mod's
//! expensive positional decisions were re-derived by every worker that touched
//! a cell. This is the seam that lets the first derivation serve the rest.
//! Instance-neutral: it reads no simulation state, so it is legal on every
//! runtime side, and entries are scoped by mod id and world seed so no mod can
//! read another's facts and no world can read a previous world's.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use mod_api::{HostCall, HostRet, MemoClaim, MEMO_MAX_KEY_BYTES, MEMO_MAX_VALUE_BYTES};

use super::guards::batch_guard;
use super::ModStoreData;

const SHARDS: usize = 64;
/// Bytes the whole store retains before it evicts oldest-first, per shard.
const SHARD_BUDGET_BYTES: usize = (128 << 20) / SHARDS;
/// How long a claimant waits for a lease holder's value before being told it
/// is pending. Nothing: a pending section parks until the holder publishes,
/// so waiting here only idled workers — a millisecond per claim added up to
/// a sixth of a habitat region's generation time.
const CLAIM_WAIT: Duration = Duration::ZERO;
/// A lease older than this belongs to a holder that trapped or never
/// published; the next claimant takes it over instead of pending forever.
const LEASE_EXPIRY: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Shard {
    entries: HashMap<Box<[u8]>, Box<[u8]>>,
    order: VecDeque<Box<[u8]>>,
    bytes: usize,
}

impl Shard {
    fn put(&mut self, key: Box<[u8]>, value: Box<[u8]>) {
        let cost = key.len() + value.len();
        match self.entries.insert(key.clone(), value) {
            Some(old) => self.bytes -= key.len() + old.len(),
            None => self.order.push_back(key),
        }
        self.bytes += cost;
        while self.bytes > SHARD_BUDGET_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(value) = self.entries.remove(&oldest) {
                self.bytes -= oldest.len() + value.len();
            }
        }
    }
}

/// A missing entry some caller is deriving right now.
struct Lease {
    started: std::time::Instant,
    published: Mutex<bool>,
    ready: Condvar,
}

impl Lease {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            published: Mutex::new(false),
            ready: Condvar::new(),
        }
    }
}

struct Store {
    shards: Box<[Mutex<Shard>]>,
    leases: Mutex<HashMap<Box<[u8]>, Arc<Lease>>>,
}

impl Store {
    fn new() -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect(),
            leases: Mutex::new(HashMap::new()),
        }
    }

    fn leases(&self) -> std::sync::MutexGuard<'_, HashMap<Box<[u8]>, Arc<Lease>>> {
        self.leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn claim(&self, key: &[u8]) -> MemoClaim {
        self.claim_with(key, CLAIM_WAIT, LEASE_EXPIRY)
    }

    /// The value, the lease to derive it, or `Pending` while another caller
    /// holds that lease past `wait`. A lease older than `expiry` passes to
    /// this caller.
    fn claim_with(&self, key: &[u8], wait: Duration, expiry: Duration) -> MemoClaim {
        if let Some(value) = self.get(key) {
            return MemoClaim::Value(value);
        }
        let held = {
            let mut leases = self.leases();
            match leases.get(key) {
                Some(lease) if lease.started.elapsed() <= expiry => Some(Arc::clone(lease)),
                _ => {
                    leases.insert(key.into(), Arc::new(Lease::new()));
                    None
                }
            }
        };
        let Some(lease) = held else {
            return MemoClaim::Lease;
        };
        {
            let published = lease
                .published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = lease
                .ready
                .wait_timeout_while(published, wait, |published| !*published)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        match self.get(key) {
            Some(value) => MemoClaim::Value(value),
            None => {
                PENDING.with(|pending| *pending.borrow_mut() = Some(key.into()));
                MemoClaim::Pending
            }
        }
    }

    fn publish(&self, key: &[u8]) {
        if let Some(lease) = self.leases().remove(key) {
            *lease
                .published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            lease.ready.notify_all();
        }
        wake_parked(key);
    }

    fn is_leased(&self, key: &[u8]) -> bool {
        self.leases()
            .get(key)
            .is_some_and(|lease| lease.started.elapsed() <= LEASE_EXPIRY)
    }
}

thread_local! {
    /// The key whose lease the most recent `Pending` claim on this thread
    /// waited on: what a deferred generation job should wait for.
    static PENDING: RefCell<Option<Box<[u8]>>> = const { RefCell::new(None) };
}

/// Forget any pending key a previous dispatch on this thread left behind.
pub(crate) fn clear_pending_key() {
    PENDING.with(|pending| *pending.borrow_mut() = None);
}

/// The key the current thread's most recent dispatch pended on, if any.
pub(crate) fn take_pending_key() -> Option<Box<[u8]>> {
    PENDING.with(|pending| pending.borrow_mut().take())
}

type Wake = Box<dyn FnOnce() + Send>;

struct Parked {
    since: Instant,
    wake: Wake,
}

/// Work waiting for a fact under derivation, woken when it publishes: a
/// section deferred on a pending lease sleeps here instead of being retried
/// while the holder is still deriving.
type ParkedByKey = HashMap<Box<[u8]>, Vec<Parked>>;
static PARKED: LazyLock<Mutex<ParkedByKey>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn parked() -> std::sync::MutexGuard<'static, ParkedByKey> {
    PARKED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run `wake` once the fact under `key` publishes. Runs it at once when the
/// fact is already there or nobody holds its lease any more.
pub(crate) fn park(key: Box<[u8]>, wake: Wake) {
    {
        let mut parked = parked();
        if STORE.get(&key).is_none() && STORE.is_leased(&key) {
            parked.entry(key).or_default().push(Parked {
                since: Instant::now(),
                wake,
            });
            return;
        }
    }
    wake();
}

fn wake_parked(key: &[u8]) {
    let woken = parked().remove(key);
    for parked in woken.into_iter().flatten() {
        (parked.wake)();
    }
}

/// Wake everything parked longer than a lease can live: its holder is gone,
/// and the retry claims the lease itself.
pub(crate) fn sweep_parked() {
    let mut stale = Vec::new();
    {
        let mut parked = parked();
        parked.retain(|_, list| {
            let (old, young): (Vec<_>, Vec<_>) = list
                .drain(..)
                .partition(|p| p.since.elapsed() > LEASE_EXPIRY);
            stale.extend(old);
            *list = young;
            !list.is_empty()
        });
    }
    for parked in stale {
        (parked.wake)();
    }
}

impl Store {
    fn shard(&self, key: &[u8]) -> std::sync::MutexGuard<'_, Shard> {
        let mut hasher = rustc_hash::FxHasher::default();
        key.hash(&mut hasher);
        self.shards[hasher.finish() as usize % SHARDS]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.shard(key).entries.get(key).map(|v| v.to_vec())
    }

    fn put(&self, key: Box<[u8]>, value: Box<[u8]>) {
        self.shard(&key).put(key, value);
    }
}

static STORE: LazyLock<Store> = LazyLock::new(Store::new);

/// The store key: the mod's own key under its id and the world seed, so a
/// fact is unreachable from any other mod or world.
fn scoped_key(mod_id: &str, seed: u32, key: &[u8]) -> Box<[u8]> {
    let mut out = Vec::with_capacity(mod_id.len() + 5 + key.len());
    out.extend_from_slice(mod_id.as_bytes());
    out.push(0);
    out.extend_from_slice(&seed.to_le_bytes());
    out.extend_from_slice(key);
    out.into_boxed_slice()
}

fn key_guard(key: &[u8]) -> Option<HostRet> {
    (key.len() > MEMO_MAX_KEY_BYTES).then(|| {
        HostRet::Error(format!(
            "memo key is {} bytes; the limit is {MEMO_MAX_KEY_BYTES}",
            key.len()
        ))
    })
}

pub(super) fn handle_memo_call(data: &ModStoreData, call: HostCall) -> HostRet {
    let seed = data.world_seed();
    match call {
        HostCall::MemoGet { key } => match key_guard(&key) {
            Some(err) => err,
            None => HostRet::Bytes(STORE.get(&scoped_key(&data.mod_id, seed, &key))),
        },
        HostCall::MemoGetMany { keys } => {
            if let Some(err) = batch_guard("MemoGetMany key", keys.len()) {
                return err;
            }
            if let Some(err) = keys.iter().find_map(|key| key_guard(key)) {
                return err;
            }
            HostRet::BytesMany(
                keys.iter()
                    .map(|key| STORE.get(&scoped_key(&data.mod_id, seed, key)))
                    .collect(),
            )
        }
        HostCall::MemoPut { key, value } => match key_guard(&key) {
            Some(err) => err,
            None if value.len() > MEMO_MAX_VALUE_BYTES => {
                STORE.publish(&scoped_key(&data.mod_id, seed, &key));
                HostRet::Bool(false)
            }
            None => {
                let key = scoped_key(&data.mod_id, seed, &key);
                STORE.put(key.clone(), value.into_boxed_slice());
                STORE.publish(&key);
                HostRet::Bool(true)
            }
        },
        HostCall::MemoClaim { key } => match key_guard(&key) {
            Some(err) => err,
            None => HostRet::MemoClaim(STORE.claim(&scoped_key(&data.mod_id, seed, &key))),
        },
        other => HostRet::Error(format!(
            "non-memo call {other:?} mis-routed to handle_memo_call (host bug)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(mod_id: &str, seed: u32, key: &[u8], value: &[u8]) -> HostRet {
        let data = ModStoreData::new(mod_id, seed);
        handle_memo_call(
            &data,
            HostCall::MemoPut {
                key: key.to_vec(),
                value: value.to_vec(),
            },
        )
    }

    fn get(mod_id: &str, seed: u32, key: &[u8]) -> Option<Vec<u8>> {
        let data = ModStoreData::new(mod_id, seed);
        match handle_memo_call(&data, HostCall::MemoGet { key: key.to_vec() }) {
            HostRet::Bytes(v) => v,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_claimant_waits_briefly_for_the_holder_then_pends_and_a_stale_lease_passes_on() {
        let data = ModStoreData::new("lease", 3);
        let claim = |data: &ModStoreData| match handle_memo_call(
            data,
            HostCall::MemoClaim {
                key: b"cell".to_vec(),
            },
        ) {
            HostRet::MemoClaim(c) => c,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            claim(&data),
            MemoClaim::Lease,
            "the first claimant holds the lease"
        );
        assert_eq!(
            claim(&data),
            MemoClaim::Pending,
            "a slow holder pends its claimants"
        );
        // A claimant still waiting when the holder publishes wakes with the
        // value; the generous wait keeps a loaded test machine from timing out.
        let key = scoped_key("lease", 3, b"cell");
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(1));
                assert_eq!(put("lease", 3, b"cell", b"v"), HostRet::Bool(true));
            });
            assert_eq!(
                STORE.claim_with(&key, Duration::from_secs(5), LEASE_EXPIRY),
                MemoClaim::Value(b"v".to_vec())
            );
        });
        assert_eq!(claim(&data), MemoClaim::Value(b"v".to_vec()));

        let key = scoped_key("lease", 3, b"dead");
        assert_eq!(
            STORE.claim_with(&key, Duration::ZERO, Duration::ZERO),
            MemoClaim::Lease
        );
        assert_eq!(
            STORE.claim_with(&key, Duration::ZERO, Duration::ZERO),
            MemoClaim::Lease,
            "an expired lease passes to the next claimant"
        );
    }

    /// Work parked on a pending fact runs when the fact publishes, at once
    /// when nothing is deriving it, and after the lease expiry when its
    /// holder never publishes.
    #[test]
    fn parked_work_wakes_on_publish_or_after_a_dead_lease() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let data = ModStoreData::new("park", 5);
        let woken = Arc::new(AtomicUsize::new(0));
        let wake = |woken: &Arc<AtomicUsize>| -> Wake {
            let woken = Arc::clone(woken);
            Box::new(move || {
                woken.fetch_add(1, Ordering::SeqCst);
            })
        };
        let key = scoped_key("park", 5, b"fact");
        assert_eq!(STORE.claim(&key), MemoClaim::Lease);
        park(key.clone(), wake(&woken));
        assert_eq!(
            woken.load(Ordering::SeqCst),
            0,
            "a live lease parks the work"
        );
        assert_eq!(
            handle_memo_call(
                &data,
                HostCall::MemoPut {
                    key: b"fact".to_vec(),
                    value: b"v".to_vec(),
                },
            ),
            HostRet::Bool(true)
        );
        assert_eq!(woken.load(Ordering::SeqCst), 1, "publishing wakes it");
        park(key.clone(), wake(&woken));
        assert_eq!(
            woken.load(Ordering::SeqCst),
            2,
            "a published fact wakes at once"
        );

        let nobody = scoped_key("park", 5, b"nobody");
        park(nobody, wake(&woken));
        assert_eq!(
            woken.load(Ordering::SeqCst),
            3,
            "an unheld fact never parks"
        );
        // The sweep only wakes work parked longer than a lease can live.
        let slow = scoped_key("park", 5, b"slow");
        assert_eq!(STORE.claim(&slow), MemoClaim::Lease);
        park(slow, wake(&woken));
        sweep_parked();
        assert_eq!(woken.load(Ordering::SeqCst), 3);
        parked().values_mut().flatten().for_each(|p| {
            p.since = Instant::now() - LEASE_EXPIRY - Duration::from_millis(1);
        });
        sweep_parked();
        assert_eq!(woken.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn entries_are_scoped_to_the_mod_and_the_world_seed() {
        assert_eq!(put("a", 1, b"k", b"v"), HostRet::Bool(true));
        assert_eq!(get("a", 1, b"k").as_deref(), Some(&b"v"[..]));
        assert_eq!(get("b", 1, b"k"), None);
        assert_eq!(get("a", 2, b"k"), None);
    }

    #[test]
    fn oversized_values_are_refused_and_the_budget_evicts_oldest_first() {
        let big = vec![7u8; MEMO_MAX_VALUE_BYTES + 1];
        assert_eq!(put("evict", 9, b"big", &big), HostRet::Bool(false));
        assert_eq!(get("evict", 9, b"big"), None);
        let value = vec![1u8; MEMO_MAX_VALUE_BYTES];
        // Fill well past one shard's budget; the first entry is the one to go.
        let n = SHARD_BUDGET_BYTES / MEMO_MAX_VALUE_BYTES * SHARDS + SHARDS;
        for i in 0..n as u32 {
            assert_eq!(
                put("evict", 9, &i.to_le_bytes(), &value),
                HostRet::Bool(true)
            );
        }
        assert!(get("evict", 9, &0u32.to_le_bytes()).is_none());
        assert!(get("evict", 9, &(n as u32 - 1).to_le_bytes()).is_some());
        for shard in STORE.shards.iter() {
            assert!(shard.lock().unwrap().bytes <= SHARD_BUDGET_BYTES);
        }
    }
}
