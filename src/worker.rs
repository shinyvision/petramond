//! Worker abstraction: off-thread cubic world generation.
//!
//! A thread pool running reused `ChunkGenerator`s. Generation is split into two job
//! kinds so it can run closest to the player one 16³ section at a time:
//!
//! - [`GenJob::Column`] — the heavy, inherently-2D part of a column (biome + density
//!   surface + feature candidate/support windows), produced once as a shared
//!   [`ColumnGen`].
//! - [`GenJob::Section`] — one 16³ section, generated cheaply from its column's shared
//!   `Arc<ColumnGen>`. These are what stream in around the player in 3D.
//!
//! The world submits jobs and drains results: `submit` requests, `try_recv` drains.

use crate::chunk::{ChunkPos, SectionPos};
use crate::section::Section;
use crate::worldgen::driver::{ChunkGenerator, ColumnGen};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

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

/// A finished generation job, drained by the world's `poll`.
pub enum GenOutput {
    /// A column's shared data; the world installs it and submits its section jobs.
    Column { pos: ChunkPos, col: Arc<ColumnGen> },
    /// A generated section, ready to install.
    Section { sp: SectionPos, section: Section },
}

/// Split the machine's cores across the three streaming background pools without
/// oversubscribing the render thread. Worldgen, light baking, and mesh building all run
/// off-thread now; if each grabbed `cores − 2` independently (as gen and light used to),
/// their threads would far outnumber the cores and the OS would preempt the render
/// thread mid-frame — the "stutters when flying" symptom. The render thread no longer
/// builds meshes, so it needs little CPU; reserve one core for it and divide the rest:
/// generation is the streaming bottleneck and gets the lion's share, meshing a moderate
/// pool, light bakes (quick) a few. Returns `(gen, light, mesh)`.
pub fn background_thread_counts() -> (usize, usize, usize) {
    let n = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    // Reserve TWO cores for the main/render thread: it still does per-frame poll + mesh
    // snapshotting + the render pass, so leaving only one lets the background pools preempt
    // it mid-frame (large, variable frame spikes). Light bakes ≈ mesh builds in total work
    // (one each per visible section), so they take an equal, smaller share and gen the rest.
    let avail = n.saturating_sub(2).max(4);
    let light = (avail / 5).clamp(2, 6);
    let mesh = (avail / 5).clamp(2, 6);
    let gen = avail.saturating_sub(light + mesh).max(2);
    (gen, light, mesh)
}

// ---------------------------------------------------------------------------
// Thread pool using std::sync::mpsc + persistent worker threads.
// ---------------------------------------------------------------------------
pub struct WorkerPool {
    tx_req: Sender<GenJob>,
    rx_res: Mutex<Receiver<GenOutput>>,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl WorkerPool {
    pub fn new(seed: u32) -> Self {
        let (tx_req, rx_req) = channel::<GenJob>();
        let (tx_res, rx_res) = channel::<GenOutput>();
        // Worldgen is the streaming bottleneck, but it shares the machine with the light
        // and mesh pools and the render thread — see [`background_thread_counts`], which
        // reserves cores so streaming work can't preempt the frame.
        let (n, _, _) = background_thread_counts();
        // The single Mutex<Receiver> is held only across the brief recv(); gen runs
        // unlocked, so contention is negligible even at high thread counts.
        let rx_req = Arc::new(Mutex::new(rx_req));
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let rx_req = rx_req.clone();
            let tx_res = tx_res.clone();
            let mut generator_seed = seed;
            let mut generator = ChunkGenerator::new(generator_seed);
            let h = thread::spawn(move || loop {
                let job = {
                    let g = rx_req.lock().unwrap();
                    g.recv()
                };
                let Ok(job) = job else { break };
                if job.seed() != generator_seed {
                    generator_seed = job.seed();
                    generator = ChunkGenerator::new(generator_seed);
                }
                // Run the (pure, generator-borrowing) job under catch_unwind so a worldgen
                // bug that panics on one section can't silently kill this worker thread and
                // shrink the pool until generation stalls. The generator is only borrowed,
                // not mutated, so it stays valid after a caught unwind.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                    GenJob::Column { pos, .. } => {
                        let t = std::time::Instant::now();
                        let col = Arc::new(generator.generate_column_gen(pos.cx, pos.cz));
                        crate::perf::GEN_COLUMN.record(t.elapsed().as_nanos() as u64);
                        GenOutput::Column { pos, col }
                    }
                    GenJob::Section { sp, col, .. } => {
                        let t = std::time::Instant::now();
                        let section = generator.generate_section(sp, &col);
                        crate::perf::GEN_SECTION.record(t.elapsed().as_nanos() as u64);
                        GenOutput::Section { sp, section }
                    }
                }));
                let out = match result {
                    Ok(out) => out,
                    Err(_) => {
                        eprintln!("worldgen worker: a generation job panicked; section skipped");
                        continue;
                    }
                };
                if tx_res.send(out).is_err() {
                    break;
                }
            });
            handles.push(h);
        }
        Self {
            tx_req,
            rx_res: Mutex::new(rx_res),
            _handles: handles,
        }
    }

    pub fn submit(&self, job: GenJob) {
        let _ = self.tx_req.send(job);
    }
    pub fn try_recv(&self) -> Option<GenOutput> {
        self.rx_res.lock().unwrap().try_recv().ok()
    }
}
