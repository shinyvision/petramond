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

use crate::chunk::{ChunkPos, SectionPos};
use crate::section::Section;
use crate::worldgen::driver::{ChunkGenerator, ColumnGen};
use std::cell::RefCell;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

/// Nice value for background worker threads. Streaming pools (jobs/save) must always
/// lose the scheduler race against the normal-priority main/render thread, so
/// saturating them during terrain streaming can never preempt a frame. Niceness —
/// unlike SCHED_IDLE — still guarantees the work makes progress on an idle machine.
const WORKER_NICE: i32 = 10;

/// Lower the CALLING thread's OS scheduling priority. Call it first thing inside each
/// background worker's thread closure.
pub(crate) fn lower_current_thread_priority() {
    #[cfg(target_os = "linux")]
    {
        // PRIO_PROCESS + a tid targets that single thread (not the whole process).
        let tid = unsafe { libc::gettid() } as u32;
        let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, tid, WORKER_NICE) };
        if rc != 0 {
            log::debug!("setpriority(nice={WORKER_NICE}) failed for worker thread");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {}
}

// ---------------------------------------------------------------------------
// The shared priority job pool.
// ---------------------------------------------------------------------------

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
    seq: AtomicU64,
}

/// The shared background pool. Owned once per [`World`](crate::world::store::World)
/// behind an `Arc`; each stage adapter (gen [`WorkerPool`], mesh, light) holds a clone
/// and submits closures with a distance-priority key.
pub struct JobPool {
    shared: Arc<PoolShared>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl JobPool {
    /// Worker count: everything but two threads reserved for the main/render thread
    /// (which still polls, snapshots, and drives the GPU every frame). The floor keeps
    /// small machines streaming. Workers also run at low OS priority (see
    /// [`lower_current_thread_priority`]), so this is sizing, not frame protection.
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
            seq: AtomicU64::new(0),
        });
        let mut handles = Vec::with_capacity(threads.max(1));
        for _ in 0..threads.max(1) {
            let shared = shared.clone();
            let h = thread::Builder::new()
                .name("llamacraft-jobs".to_string())
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

    /// Queue `f` at `key` (lower runs sooner; equal keys run FIFO).
    pub fn submit<F: FnOnce() + Send + 'static>(&self, key: i64, f: F) {
        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed);
        let job = QueuedJob {
            key,
            seq,
            run: Box::new(f),
        };
        self.shared.queue.lock().unwrap().push(job);
        self.shared.available.notify_one();
    }
}

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
}

thread_local! {
    /// Per-worker reused generator. Building a `ChunkGenerator` sets up the full noise
    /// stack, far too heavy per job; per-thread reuse also keeps its column-noise cache
    /// warm across the jobs of one streaming burst.
    static GENERATOR: RefCell<Option<(u32, ChunkGenerator)>> = const { RefCell::new(None) };
}

fn run_gen_job(job: GenJob) -> GenOutput {
    GENERATOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let seed = job.seed();
        if slot.as_ref().is_none_or(|(s, _)| *s != seed) {
            *slot = Some((seed, ChunkGenerator::new(seed)));
        }
        let (_, generator) = slot.as_mut().expect("generator installed above");
        match job {
            GenJob::Column { pos, .. } => GenOutput::Column {
                pos,
                col: Arc::new(generator.generate_column_gen(pos.cx, pos.cz)),
            },
            GenJob::Section { sp, col, .. } => GenOutput::Section {
                sp,
                section: Arc::new(generator.generate_section(sp, &col)),
            },
        }
    })
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

    pub fn submit(&self, key: i64, job: GenJob) {
        let tx = self.tx_res.clone();
        self.pool.submit(key, move || {
            let _ = tx.send(run_gen_job(job));
        });
    }

    pub fn try_recv(&self) -> Option<GenOutput> {
        self.rx_res.lock().unwrap().try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowers_priority_on_the_calling_thread() {
        thread::spawn(|| {
            lower_current_thread_priority();
            #[cfg(target_os = "linux")]
            {
                let tid = unsafe { libc::gettid() } as u32;
                let prio = unsafe { libc::getpriority(libc::PRIO_PROCESS, tid) };
                assert_eq!(prio, WORKER_NICE);
            }
        })
        .join()
        .unwrap();
    }

    #[test]
    fn pool_runs_lowest_key_first_and_fifo_on_ties() {
        // One worker so execution order IS pop order.
        let pool = JobPool::new(1);
        let order = Arc::new(Mutex::new(Vec::new()));
        // Park the worker on a first job so the rest queue up behind it and get
        // priority-ordered rather than raced one-by-one.
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        {
            let gate = gate.clone();
            pool.submit(i64::MIN, move || {
                let (lock, cv) = &*gate;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            });
        }
        for (key, tag) in [(50, "far"), (10, "near-a"), (10, "near-b"), (30, "mid")] {
            let order = order.clone();
            pool.submit(key, move || order.lock().unwrap().push(tag));
        }
        // Largest key = runs last; signals that everything before it completed
        // (dropping the pool discards unstarted jobs, so wait before dropping).
        let (done_tx, done_rx) = channel::<()>();
        pool.submit(i64::MAX, move || {
            let _ = done_tx.send(());
        });
        {
            let (lock, cv) = &*gate;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
        done_rx.recv().unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["near-a", "near-b", "mid", "far"]);
    }
}
