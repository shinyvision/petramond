//! Background job pool: ONE priority-ordered thread pool shared by every streaming
//! stage — worldgen (columns + sections), light bakes, and mesh builds.
//!
//! The stages used to run on three fixed pools sized by a static core split, which
//! left most threads idle whenever the streaming mix shifted (gen-heavy while flying
//! into new terrain, light/mesh-heavy right after). One shared pool means whichever
//! stage has work gets the whole machine, and one shared PRIORITY queue means the
//! nearest work runs first ACROSS stages, not merely within each stage: a near
//! section's whole ladder (gen → light → mesh) outranks far terrain.
//!
//! Priorities are the streamer's distance keys (`LoadTarget::{column,section}_priority_key`,
//! lower = sooner), which share one scale so keys from different stages compare
//! meaningfully. Ties run FIFO via a submission sequence number.

use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::Section;
use petramond_worldgen::driver::{ChunkGenerator, ColumnGen};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

#[derive(Clone)]
pub struct JobCancel(Arc<AtomicBool>);

impl Default for JobCancel {
    fn default() -> Self {
        Self::new()
    }
}

impl JobCancel {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub fn same_job(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Identifies one submission, including after its execution has finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobTicket(u64);

static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

struct QueuedJob {
    key: i64,
    seq: u64,
    run: Box<dyn FnOnce() + Send>,
}

// BinaryHeap is a max-heap; invert the comparison so the SMALLEST (key, seq) —
// nearest first, then FIFO — pops first.
impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.key, other.seq).cmp(&(self.key, self.seq))
    }
}
impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Eq for QueuedJob {}
impl PartialEq for QueuedJob {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.seq == other.seq
    }
}

struct PoolShared {
    queue: Mutex<BinaryHeap<QueuedJob>>,
    available: Condvar,
    shutdown: AtomicBool,
}

/// The shared background pool. Owned once per `World`
/// behind an `Arc`; each stage adapter (gen [`WorkerPool`], mesh, light) holds a clone
/// and submits closures with a distance-priority key.
pub struct JobPool {
    shared: Arc<PoolShared>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl JobPool {
    pub fn default_threads() -> usize {
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        n.saturating_sub(2).max(4)
    }

    pub fn new(threads: usize) -> Self {
        let shared = Arc::new(PoolShared {
            queue: Mutex::new(BinaryHeap::new()),
            available: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        // `threads == 0` is INLINE mode (see [`inline`](Self::inline)): no
        // workers at all, so `submit` can never queue — it runs on the caller.
        let mut handles = Vec::with_capacity(threads);
        for _ in 0..threads {
            let shared = shared.clone();
            let h = thread::Builder::new()
                .name("petramond-jobs".to_string())
                .spawn(move || {
                    lower_current_thread_priority();
                    loop {
                        let job = {
                            let mut q = shared.queue.lock().unwrap();
                            loop {
                                if let Some(job) = q.pop() {
                                    break Some(job);
                                }
                                if shared.shutdown.load(Ordering::Relaxed) {
                                    break None;
                                }
                                q = shared.available.wait(q).unwrap();
                            }
                        };
                        let Some(job) = job else { break };
                        // catch_unwind so one panicking job (e.g. a worldgen bug on one
                        // section) can't silently kill a worker and shrink the pool
                        // until streaming stalls. Jobs are pure over owned/Arc inputs,
                        // so a caught unwind leaves no broken shared state behind.
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.run)).is_err()
                        {
                            eprintln!("background job panicked; result skipped");
                        }
                    }
                })
                .expect("spawn job pool worker");
            handles.push(h);
        }
        Self { shared, handles }
    }

    /// A pool with NO worker threads: [`submit`](Self::submit) runs each job
    /// immediately on the caller, in submission order. The in-process test
    /// harness uses this so a pump that queues gen/light/mesh work FINISHES it
    /// before returning — tests then gate on one more pump, never on
    /// wall-clock sleeps racing a (possibly CPU-starved) worker pool, which is
    /// what made the streaming test class slow and intermittently flaky.
    pub fn inline() -> Self {
        Self::new(0)
    }

    /// Whether this pool runs jobs on the calling thread (see [`inline`](Self::inline)).
    pub fn is_inline(&self) -> bool {
        self.handles.is_empty()
    }

    /// Queue `f` at `key` (lower runs sooner; equal keys run FIFO). On an
    /// INLINE pool the job runs immediately, inline with this call.
    pub fn submit<F: FnOnce() + Send + 'static>(&self, key: i64, f: F) -> JobTicket {
        let seq = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let ticket = JobTicket(seq);
        if self.is_inline() {
            f();
            return ticket;
        }
        let job = QueuedJob {
            key,
            seq,
            run: Box::new(f),
        };
        self.shared.queue.lock().unwrap().push(job);
        self.shared.available.notify_one();
        ticket
    }

    /// Change priorities of waiting jobs without restarting work. Finished and
    /// running tickets are ignored; equal priorities retain submission order.
    pub fn reprioritize(&self, updates: impl IntoIterator<Item = (JobTicket, i64)>) {
        let updates: FxHashMap<_, _> = updates.into_iter().collect();
        if updates.is_empty() {
            return;
        }
        let mut queue = self.shared.queue.lock().unwrap();
        let mut jobs = std::mem::take(&mut *queue).into_vec();
        for job in &mut jobs {
            if let Some(&key) = updates.get(&JobTicket(job.seq)) {
                job.key = key;
            }
        }
        *queue = jobs.into();
    }

    /// Remove only jobs that have not started. The returned tickets identify
    /// exactly the requests whose owners can now release their pending slots.
    pub fn remove_queued(
        &self,
        tickets: impl IntoIterator<Item = JobTicket>,
    ) -> FxHashSet<JobTicket> {
        let tickets: FxHashSet<_> = tickets.into_iter().collect();
        let mut removed = Vec::new();
        if !tickets.is_empty() {
            let mut queue = self.shared.queue.lock().unwrap();
            let jobs = std::mem::take(&mut *queue).into_vec();
            let (discard, keep): (Vec<_>, Vec<_>) = jobs
                .into_iter()
                .partition(|job| tickets.contains(&JobTicket(job.seq)));
            removed = discard;
            *queue = keep.into();
        }
        removed.iter().map(|job| JobTicket(job.seq)).collect()
    }
}

#[cfg(target_os = "linux")]
pub fn lower_current_thread_priority() {
    // Background throughput should not preempt the render or simulation owner
    // threads. Failure is harmless (for example under a restrictive sandbox).
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        let _ = libc::setpriority(libc::PRIO_PROCESS, tid, 5);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn lower_current_thread_priority() {}

impl Drop for JobPool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        // Drop queued-but-unstarted work so shutdown doesn't generate a world nobody
        // will see; in-flight jobs finish (they hold snapshots, not world borrows).
        self.shared.queue.lock().unwrap().clear();
        self.shared.available.notify_all();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Worldgen stage adapter.
// ---------------------------------------------------------------------------

/// A unit of off-thread generation. Both variants carry the world `seed` so a worker
/// can rebuild its (immutable, seed-derived) generator if the world is reseeded.
pub enum GenJob {
    /// Compute one column's shared 2D data.
    Column { pos: ChunkPos, seed: u32 },
    /// Generate one 16³ section from its column's shared data.
    Section {
        sp: SectionPos,
        col: Arc<ColumnGen>,
        seed: u32,
    },
}

impl GenJob {
    #[inline]
    fn seed(&self) -> u32 {
        match self {
            GenJob::Column { seed, .. } | GenJob::Section { seed, .. } => *seed,
        }
    }
}

/// A finished generation job, drained by the world's `poll`. Both payloads ride
/// behind `Arc` — the world stores them as `Arc` anyway, and it keeps the enum small
/// through the result channel.
pub enum GenOutput {
    /// A column's shared data; the world installs it and submits its section jobs.
    Column { pos: ChunkPos, col: Arc<ColumnGen> },
    /// A generated section, ready to install.
    Section {
        sp: SectionPos,
        section: Arc<Section>,
    },
    /// A hook deferred the section: a positional fact it depends on is being
    /// derived by another worker. The owner submits the job again later (the
    /// column data rides along so it can); the eventual section does not
    /// depend on when.
    SectionDeferred { sp: SectionPos, col: Arc<ColumnGen> },
    /// The column job panicked (a worldgen bug at these coordinates). Reported —
    /// not silently dropped — so the streamer clears its pending flag: a leaked
    /// flag left the column permanently ungenerated (an invisible hole) and
    /// permanently in-flight (freezing the sim guard around it).
    ColumnFailed(ChunkPos),
    /// The section job panicked; same contract as [`GenOutput::ColumnFailed`].
    SectionFailed(SectionPos),
}

thread_local! {
    /// Per-worker reused generator. Building a `ChunkGenerator` sets up the full noise
    /// stack, far too heavy per job; per-thread reuse also keeps its column-noise cache
    /// warm across the jobs of one streaming burst. Keyed by `(seed, gen-hook epoch)`
    /// so a session (re)installing mod worldgen hooks evicts generators that captured
    /// the previous config.
    static GENERATOR: RefCell<Option<((u32, u64), ChunkGenerator)>> = const { RefCell::new(None) };
}

fn run_gen_job(job: GenJob) -> GenOutput {
    crate::modding::clear_pending_key();
    GENERATOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let seed = job.seed();
        let key = (seed, crate::modding::gen::installed_epoch());
        if slot.as_ref().is_none_or(|(k, _)| *k != key) {
            *slot = Some((key, ChunkGenerator::new(seed)));
        }
        let (_, generator) = slot.as_mut().expect("generator installed above");
        match job {
            GenJob::Column { pos, .. } => GenOutput::Column {
                pos,
                col: Arc::new(generator.generate_column_gen(pos.cx, pos.cz)),
            },
            GenJob::Section { sp, col, .. } => match generator.try_generate_section(sp, &col) {
                Some(section) => GenOutput::Section {
                    sp,
                    section: Arc::new(section),
                },
                None => GenOutput::SectionDeferred { sp, col },
            },
        }
    })
}

/// Fan a set of 16×16 surface-tile warmups across the pool at maximum
/// priority: each job fills one tile of the shared feature-window memo
/// through the worker's thread-local generator. Fire-and-forget — the memo
/// is the output. Session bootstrap uses this to compute the spawn area's
/// tiles in parallel while the rest of construction runs, instead of the
/// first column job deriving them serially on one worker.
pub fn warm_surface_tiles(pool: &JobPool, seed: u32, tiles: impl IntoIterator<Item = (i32, i32)>) {
    for (tcx, tcz) in tiles {
        pool.submit(i64::MIN, move || {
            GENERATOR.with(|slot| {
                let mut slot = slot.borrow_mut();
                let key = (seed, crate::modding::gen::installed_epoch());
                if slot.as_ref().is_none_or(|(k, _)| *k != key) {
                    *slot = Some((key, ChunkGenerator::new(seed)));
                }
                let (_, generator) = slot.as_mut().expect("generator installed above");
                generator.warm_surface_tile(tcx, tcz);
            });
        });
    }
}

/// Cancellation and queue identity for one generation request.
pub struct GenJobHandle {
    cancel: JobCancel,
    pub(crate) ticket: JobTicket,
}

impl GenJobHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Gen-stage adapter over the shared [`JobPool`]: `submit` queues generation at a
/// distance priority, `try_recv` drains finished outputs on the main thread.
pub struct WorkerPool {
    pool: Arc<JobPool>,
    tx_res: Sender<GenOutput>,
    rx_res: Mutex<Receiver<GenOutput>>,
}

impl WorkerPool {
    pub fn new(pool: Arc<JobPool>) -> Self {
        let (tx_res, rx_res) = channel::<GenOutput>();
        Self {
            pool,
            tx_res,
            rx_res: Mutex::new(rx_res),
        }
    }

    pub fn submit(&self, key: i64, job: GenJob) -> GenJobHandle {
        let cancel = JobCancel::new();
        let ticket = spawn_gen(
            &self.pool,
            key,
            job,
            self.tx_res.clone(),
            cancel.clone(),
            false,
        );
        GenJobHandle { cancel, ticket }
    }

    pub(crate) fn reprioritize(&self, updates: impl IntoIterator<Item = (JobTicket, i64)>) {
        self.pool.reprioritize(updates);
    }

    pub(crate) fn remove_queued(
        &self,
        tickets: impl IntoIterator<Item = JobTicket>,
    ) -> FxHashSet<JobTicket> {
        self.pool.remove_queued(tickets)
    }

    pub fn try_recv(&self) -> Option<GenOutput> {
        sweep_parked();
        self.rx_res.lock().unwrap().try_recv().ok()
    }
}

/// Queue one generation job. A section deferred on a fact another worker is
/// deriving is PARKED on that fact and queued again when it publishes, so
/// the workers never spin on it; a `resumed` job that is cancelled meanwhile
/// still reports, because its owner holds no ticket for it.
fn spawn_gen(
    pool: &Arc<JobPool>,
    key: i64,
    job: GenJob,
    tx: Sender<GenOutput>,
    cancel: JobCancel,
    resumed: bool,
) -> JobTicket {
    let failed = match &job {
        GenJob::Column { pos, .. } => GenOutput::ColumnFailed(*pos),
        GenJob::Section { sp, .. } => GenOutput::SectionFailed(*sp),
    };
    let seed = job.seed();
    let pool_again = Arc::clone(pool);
    let inline = pool.is_inline();
    pool.submit(key, move || {
        if cancel.is_cancelled() {
            if resumed {
                if let GenJob::Section { sp, col, .. } = job {
                    let _ = tx.send(GenOutput::SectionDeferred { sp, col });
                }
            }
            return;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_gen_job(job))) {
            Ok(GenOutput::SectionDeferred { sp, col }) => {
                match crate::modding::take_pending_key().filter(|_| !inline) {
                    Some(pending) => crate::modding::park(
                        pending,
                        Box::new(move || {
                            spawn_gen(
                                &pool_again,
                                key,
                                GenJob::Section { sp, col, seed },
                                tx,
                                cancel,
                                true,
                            );
                        }),
                    ),
                    None => {
                        let _ = tx.send(GenOutput::SectionDeferred { sp, col });
                    }
                }
            }
            Ok(out) => {
                let _ = tx.send(out);
            }
            Err(_) => {
                // The per-thread generator may be mid-mutation; rebuild it
                // before the next job rather than trusting its caches.
                GENERATOR.with(|slot| *slot.borrow_mut() = None);
                eprintln!("worldgen job panicked; reporting failure");
                let _ = tx.send(failed);
            }
        }
    })
}

/// Parked jobs whose fact never published wake after the lease expiry; the
/// sweep runs from the owner's poll, throttled to a few times a second.
fn sweep_parked() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    static EPOCH: std::sync::LazyLock<std::time::Instant> =
        std::sync::LazyLock::new(std::time::Instant::now);
    let now = EPOCH.elapsed().as_millis() as u64;
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= 250
        && LAST
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        crate::modding::sweep_parked();
    }
}

#[cfg(test)]
mod tests;
