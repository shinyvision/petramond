//! One priority-ordered thread pool shared by worldgen (columns + sections), light bakes, and mesh
//! builds.
//!
//! Whichever stage has work gets the whole machine as the streaming mix shifts
//! between generation, lighting and meshing. One shared priority queue means nearest
//! work wins across stages, not just within a stage: a near section's whole gen→light→mesh ladder
//! beats far terrain.
//!
//! Priorities come from the streamer's distance keys (`LoadTarget::{column,section}_priority_key`,
//! lower = sooner), sharing one scale so stages compare directly. Ties go FIFO by submission
//! sequence number.

use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::Section;
use petramond_worldgen::{ChunkGenerator, ColumnGen, PendingSection, SectionGen};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::collections::BTreeMap;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobTicket(u64);

static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

type Job = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct JobQueue {
    order: BTreeMap<(i64, u64), Job>,
    keys: FxHashMap<u64, i64>,
}

impl JobQueue {
    fn push(&mut self, key: i64, seq: u64, run: Job) {
        self.keys.insert(seq, key);
        self.order.insert((key, seq), run);
    }

    fn pop(&mut self) -> Option<Job> {
        let ((_, seq), run) = self.order.pop_first()?;
        self.keys.remove(&seq);
        Some(run)
    }

    fn rekey(&mut self, seq: u64, key: i64) {
        let Some(old) = self.keys.get_mut(&seq) else {
            return;
        };
        if *old == key {
            return;
        }
        let run = self
            .order
            .remove(&(*old, seq))
            .expect("job index and order stay in step");
        *old = key;
        self.order.insert((key, seq), run);
    }

    fn remove(&mut self, seq: u64) -> Option<Job> {
        let key = self.keys.remove(&seq)?;
        self.order.remove(&(key, seq))
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        debug_assert_eq!(self.order.len(), self.keys.len());
        self.order.len()
    }
}

struct PoolShared {
    queue: Mutex<JobQueue>,
    available: Condvar,
    shutdown: AtomicBool,
}

fn run_contained(run: Job) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).is_err() {
        eprintln!("background job panicked; its owner was sent a failure result");
    }
}

pub struct ReportSlot<T: Send + 'static> {
    tx: Sender<T>,
    failure: Option<T>,
    stage: &'static str,
    at: SectionPos,
}

impl<T: Send + 'static> ReportSlot<T> {
    pub fn new(tx: Sender<T>, failure: T, stage: &'static str, at: SectionPos) -> Self {
        Self {
            tx,
            failure: Some(failure),
            stage,
            at,
        }
    }

    pub fn complete(mut self, result: T) {
        self.failure = None;
        let _ = self.tx.send(result);
    }
}

impl<T: Send + 'static> Drop for ReportSlot<T> {
    fn drop(&mut self) {
        let Some(failure) = self.failure.take() else {
            return;
        };
        if thread::panicking() {
            eprintln!(
                "{} job for section {:?} panicked; reporting failure",
                self.stage, self.at
            );
        }
        let _ = self.tx.send(failure);
    }
}

pub struct JobPool {
    shared: Arc<PoolShared>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl JobPool {
    pub fn default_threads() -> usize {
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        n.saturating_sub(2).max(2).min(n)
    }

    pub fn new(threads: usize) -> Self {
        let shared = Arc::new(PoolShared {
            queue: Mutex::new(JobQueue::default()),
            available: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
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
                        run_contained(job);
                    }
                })
                .expect("spawn job pool worker");
            handles.push(h);
        }
        Self { shared, handles }
    }

    pub fn inline() -> Self {
        Self::new(0)
    }

    pub fn is_inline(&self) -> bool {
        self.handles.is_empty()
    }

    pub fn submit<F: FnOnce() + Send + 'static>(&self, key: i64, f: F) -> JobTicket {
        let seq = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let ticket = JobTicket(seq);
        if self.is_inline() {
            run_contained(Box::new(f));
            return ticket;
        }
        let content = petramond_world::content::Content::current();
        self.shared.queue.lock().unwrap().push(
            key,
            seq,
            Box::new(move || {
                let _pin = petramond_world::content::pin(content);
                f()
            }),
        );
        self.shared.available.notify_one();
        ticket
    }

    pub fn reprioritize(&self, updates: impl IntoIterator<Item = (JobTicket, i64)>) {
        let mut updates = updates.into_iter().peekable();
        if updates.peek().is_none() {
            return;
        }
        let mut queue = self.shared.queue.lock().unwrap();
        for (ticket, key) in updates {
            queue.rekey(ticket.0, key);
        }
    }

    pub fn remove_queued(
        &self,
        tickets: impl IntoIterator<Item = JobTicket>,
    ) -> FxHashSet<JobTicket> {
        let mut removed = FxHashSet::default();
        let mut discarded = Vec::new();
        {
            let mut queue = self.shared.queue.lock().unwrap();
            for ticket in tickets {
                if let Some(job) = queue.remove(ticket.0) {
                    removed.insert(ticket);
                    discarded.push(job);
                }
            }
        }
        drop(discarded);
        removed
    }

    #[cfg(test)]
    fn queued_len(&self) -> usize {
        self.shared.queue.lock().unwrap().len()
    }
}

#[cfg(target_os = "linux")]
pub fn lower_current_thread_priority() {
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
        let discarded = std::mem::take(&mut *self.shared.queue.lock().unwrap());
        drop(discarded);
        self.shared.available.notify_all();
        let me = std::thread::current().id();
        for h in self.handles.drain(..) {
            if h.thread().id() != me {
                let _ = h.join();
            }
        }
    }
}

pub enum GenJob {
    Column {
        pos: ChunkPos,
        seed: u32,
    },
    Section {
        sp: SectionPos,
        col: Arc<ColumnGen>,
        seed: u32,
    },
    ResumeSection {
        pending: Box<PendingSection>,
        col: Arc<ColumnGen>,
        seed: u32,
    },
}

impl GenJob {
    #[inline]
    fn seed(&self) -> u32 {
        match self {
            GenJob::Column { seed, .. }
            | GenJob::Section { seed, .. }
            | GenJob::ResumeSection { seed, .. } => *seed,
        }
    }
}

pub enum GenOutput {
    Column {
        pos: ChunkPos,
        col: Arc<ColumnGen>,
    },
    Section {
        sp: SectionPos,
        section: Arc<Section>,
    },
    SectionDeferred {
        sp: SectionPos,
        col: Arc<ColumnGen>,
        pending: Box<PendingSection>,
    },
    ColumnFailed(ChunkPos),
    SectionFailed(SectionPos),
}

type GeneratorKey = (u32, (u64, u64));
type CachedGenerator = RefCell<Option<(GeneratorKey, ChunkGenerator)>>;

thread_local! {
    /// Per-worker reused generator. Building a `ChunkGenerator` sets up the full noise
    /// stack, far too heavy per job; per-thread reuse also keeps its column-noise cache
    /// warm across the jobs of one streaming burst. Keyed by the seed and the installed
    /// config (gen-hook and memo epochs), so a session (re)installing mod worldgen hooks
    /// or a new world installing its caches evicts generators that captured the old ones.
    static GENERATOR: CachedGenerator =
        const { RefCell::new(None) };
}

fn run_gen_job(job: GenJob) -> GenOutput {
    crate::modding::clear_pending_key();
    GENERATOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let seed = job.seed();
        let key = (seed, ChunkGenerator::installed_config());
        if slot.as_ref().is_none_or(|(k, _)| *k != key) {
            *slot = Some((key, ChunkGenerator::new(seed)));
        }
        let (_, generator) = slot.as_mut().expect("generator installed above");
        match job {
            GenJob::Column { pos, .. } => GenOutput::Column {
                pos,
                col: Arc::new(generator.generate_column_gen(pos.cx, pos.cz)),
            },
            GenJob::Section { sp, col, .. } => {
                section_output(sp, generator.start_section(sp, &col), col)
            }
            GenJob::ResumeSection { pending, col, .. } => {
                section_output(pending.pos(), generator.resume_section(*pending, &col), col)
            }
        }
    })
}

fn section_output(sp: SectionPos, attempt: SectionGen, col: Arc<ColumnGen>) -> GenOutput {
    match attempt {
        SectionGen::Ready(section) => GenOutput::Section {
            sp,
            section: Arc::new(section),
        },
        SectionGen::Deferred(pending) => GenOutput::SectionDeferred {
            sp,
            col,
            pending: Box::new(pending),
        },
    }
}

pub fn warm_surface_tiles(pool: &JobPool, seed: u32, tiles: impl IntoIterator<Item = (i32, i32)>) {
    for (tcx, tcz) in tiles {
        pool.submit(i64::MIN, move || {
            GENERATOR.with(|slot| {
                let mut slot = slot.borrow_mut();
                let key = (seed, ChunkGenerator::installed_config());
                if slot.as_ref().is_none_or(|(k, _)| *k != key) {
                    *slot = Some((key, ChunkGenerator::new(seed)));
                }
                let (_, generator) = slot.as_mut().expect("generator installed above");
                generator.warm_surface_tile(tcx, tcz);
            });
        });
    }
}

pub struct GenJobHandle {
    cancel: JobCancel,
    pub(crate) ticket: JobTicket,
}

impl GenJobHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

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
        GenJob::ResumeSection { pending, .. } => GenOutput::SectionFailed(pending.pos()),
    };
    let seed = job.seed();
    let pool_again = Arc::clone(pool);
    let inline = pool.is_inline();
    pool.submit(key, move || {
        if cancel.is_cancelled() {
            if resumed {
                if let GenJob::ResumeSection { pending, col, .. } = job {
                    let _ = tx.send(GenOutput::SectionDeferred {
                        sp: pending.pos(),
                        col,
                        pending,
                    });
                }
            }
            return;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_gen_job(job))) {
            Ok(GenOutput::SectionDeferred { sp, col, pending }) => {
                match crate::modding::take_pending_key().filter(|_| !inline) {
                    Some(fact) => crate::modding::park(
                        fact,
                        Box::new(move || {
                            spawn_gen(
                                &pool_again,
                                key,
                                GenJob::ResumeSection { pending, col, seed },
                                tx,
                                cancel,
                                true,
                            );
                        }),
                    ),
                    None => {
                        let _ = tx.send(GenOutput::SectionDeferred { sp, col, pending });
                    }
                }
            }
            Ok(out) => {
                let _ = tx.send(out);
            }
            Err(_) => {
                GENERATOR.with(|slot| *slot.borrow_mut() = None);
                eprintln!("worldgen job panicked; reporting failure");
                let _ = tx.send(failed);
            }
        }
    })
}

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
