//! Shared light -> mesh bundles for local predicted terrain edits.
//!
//! The replica remains the sole owner of live sections. Initial prediction
//! runs an owned snapshot bundle synchronously; reconciliation runs that same
//! bundle on a worker. Freshly baked cubes patch the mesh snapshots before the
//! build, and the owner installs a revision-fresh result atomically.

use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use crate::chunk::SectionPos;
use crate::mesh::ChunkMesh;
use crate::worker::{JobCancel, JobPool};

use super::light::{run_light_bake, LightBakeJob, LightBakeResult};
use super::mesh_pool::{self, MeshJob};

const PREDICTION_TERRAIN_PRIORITY: i64 = i64::MIN;

/// Cap on pool helper tasks fanned out per bundle stage. Extra helpers beyond
/// the item count or the pool width only cost a queue slot and an idle wake.
const MAX_BATCH_HELPERS: usize = 16;

#[derive(Copy, Clone)]
pub(super) struct SectionGuard {
    pub pos: SectionPos,
    pub light_revision: u64,
    pub mesh_revision: u64,
}

pub(super) enum PredictionMeshJob {
    Build(MeshJob),
    Remove { pos: SectionPos, revision: u64 },
}

impl PredictionMeshJob {
    fn pos(&self) -> SectionPos {
        match self {
            Self::Build(job) => job.pos,
            Self::Remove { pos, .. } => *pos,
        }
    }
}

pub(super) enum PredictionMeshResult {
    Built {
        pos: SectionPos,
        revision: u64,
        mesh: ChunkMesh,
    },
    Remove {
        pos: SectionPos,
        revision: u64,
    },
}

impl PredictionMeshResult {
    pub(super) fn pos(&self) -> SectionPos {
        match self {
            Self::Built { pos, .. } | Self::Remove { pos, .. } => *pos,
        }
    }

    pub(super) fn revision(&self) -> u64 {
        match self {
            Self::Built { revision, .. } | Self::Remove { revision, .. } => *revision,
        }
    }
}

/// One candidate light bake plus the cubes it would replace, so the runner
/// can diff the fresh bake against them and prune meshes nothing sampled.
pub(super) struct PredictionLightJob {
    pub job: LightBakeJob,
    pub prev_skylight: Option<Arc<[u8]>>,
    pub prev_blocklight: Option<Arc<[u8]>>,
}

pub(super) struct PredictionLightResult {
    pub result: LightBakeResult,
    /// [`super::light::cube_region_changes`] mask vs. the pre-bake cubes:
    /// `0` = byte-identical rebake (install settles the flag only).
    pub mask: u32,
    /// The section had no baked cubes before this bundle; its sampling
    /// neighbours were parked on its `light_dirty`, so no rim requeue applies.
    pub first_bake: bool,
}

pub(super) struct PredictionTerrainResult {
    pub guards: Vec<SectionGuard>,
    pub lights: Vec<PredictionLightResult>,
    pub meshes: Vec<PredictionMeshResult>,
}

pub(super) struct PredictionTerrainWork {
    pub guards: Vec<SectionGuard>,
    pub lights: Vec<PredictionLightJob>,
    pub meshes: Vec<PredictionMeshJob>,
    /// Sections whose mesh pads sample an edited cell: their geometry/AO
    /// changed, so they rebuild regardless of what the light diff says.
    pub always_mesh: Vec<SectionPos>,
}

struct PendingPredictionTerrain {
    cancel: JobCancel,
    affected: Box<[SectionPos]>,
    light_positions: Box<[SectionPos]>,
    mesh_positions: Box<[SectionPos]>,
}

pub(super) struct PredictionTerrainCompletion {
    pub result: Option<PredictionTerrainResult>,
    pub mesh_positions: Box<[SectionPos]>,
}

struct PredictionTerrainEnvelope {
    id: u64,
    result: Option<PredictionTerrainResult>,
}

pub(super) struct PredictionTerrainQueue {
    pool: Arc<JobPool>,
    tx: Sender<PredictionTerrainEnvelope>,
    rx: Receiver<PredictionTerrainEnvelope>,
    pending: FxHashMap<u64, PendingPredictionTerrain>,
    next_id: u64,
}

impl PredictionTerrainQueue {
    pub(super) fn new(pool: Arc<JobPool>) -> Self {
        let (tx, rx) = channel();
        Self {
            pool,
            tx,
            rx,
            pending: FxHashMap::default(),
            next_id: 1,
        }
    }

    pub(super) fn submit(&mut self, work: PredictionTerrainWork) -> Vec<SectionPos> {
        let light_positions: Box<[SectionPos]> =
            work.lights.iter().map(|light| light.job.pos()).collect();
        let mesh_positions: Box<[SectionPos]> =
            work.meshes.iter().map(PredictionMeshJob::pos).collect();
        let mut affected: Vec<SectionPos> = work.guards.iter().map(|guard| guard.pos).collect();
        for &pos in mesh_positions.iter() {
            if !affected.contains(&pos) {
                affected.push(pos);
            }
        }

        // A newer edit whose sampled region overlaps an older job contains the
        // newer world snapshot. Retire the older work instead of spending CPU
        // on a result its revision guards could never install.
        let requeue = self.cancel_overlapping(&affected);

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let cancel = JobCancel::new();
        let worker_cancel = cancel.clone();
        let tx = self.tx.clone();
        let pool = Arc::clone(&self.pool);
        self.pool.submit(PREDICTION_TERRAIN_PRIORITY, move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_prediction_terrain(work, &worker_cancel, &pool)
            }))
            .ok()
            .flatten();
            let _ = tx.send(PredictionTerrainEnvelope { id, result });
        });
        self.pending.insert(
            id,
            PendingPredictionTerrain {
                cancel,
                affected: affected.into_boxed_slice(),
                light_positions,
                mesh_positions,
            },
        );
        requeue
    }

    pub(super) fn try_recv(&mut self) -> Option<PredictionTerrainCompletion> {
        while let Ok(envelope) = self.rx.try_recv() {
            let Some(pending) = self.pending.remove(&envelope.id) else {
                continue;
            };
            return Some(PredictionTerrainCompletion {
                result: envelope.result,
                mesh_positions: pending.mesh_positions,
            });
        }
        None
    }

    pub(super) fn owns_light(&self, pos: SectionPos) -> bool {
        self.pending
            .values()
            .any(|pending| pending.light_positions.contains(&pos))
    }

    pub(super) fn owns_mesh(&self, pos: SectionPos) -> bool {
        self.pending
            .values()
            .any(|pending| pending.mesh_positions.contains(&pos))
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn cancel_section(&mut self, pos: SectionPos) {
        let _ = self.cancel_overlapping(&[pos]);
    }

    pub(super) fn cancel_overlapping(&mut self, positions: &[SectionPos]) -> Vec<SectionPos> {
        let cancelled: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, pending)| positions.iter().any(|pos| pending.affected.contains(pos)))
            .map(|(&id, _)| id)
            .collect();
        let mut requeue = Vec::new();
        for id in cancelled {
            if let Some(pending) = self.pending.remove(&id) {
                pending.cancel.cancel();
                for &pos in pending.mesh_positions.iter() {
                    if !requeue.contains(&pos) {
                        requeue.push(pos);
                    }
                }
            }
        }
        requeue
    }

    pub(super) fn cancel_all(&mut self) {
        for (_, pending) in self.pending.drain() {
            pending.cancel.cancel();
        }
    }

    /// The shared streaming pool, for the synchronous initial-prediction
    /// caller to fan its bundle out over.
    pub(super) fn pool(&self) -> &Arc<JobPool> {
        &self.pool
    }
}

/// Run the exact worker bundle on the calling thread, fanning its independent
/// items across the shared pool. Initial local prediction uses this
/// deliberately so the edit returns only after its predicted light and meshes
/// have been installed; reconciliation submits the same work through
/// [`PredictionTerrainQueue`].
pub(super) fn run_prediction_terrain_synchronously(
    work: PredictionTerrainWork,
    pool: &Arc<JobPool>,
) -> Option<PredictionTerrainResult> {
    run_prediction_terrain(work, &JobCancel::new(), pool)
}

fn run_prediction_terrain(
    work: PredictionTerrainWork,
    cancel: &JobCancel,
    pool: &Arc<JobPool>,
) -> Option<PredictionTerrainResult> {
    let PredictionTerrainWork {
        guards,
        lights,
        meshes,
        always_mesh,
    } = work;
    let light_results = run_parallel(pool, cancel, lights, |light| {
        let PredictionLightJob {
            job,
            prev_skylight,
            prev_blocklight,
        } = light;
        let first_bake = prev_skylight.is_none();
        let result = run_light_bake(job);
        let mask = if first_bake {
            super::light::REGION_ALL
        } else {
            super::light::cube_region_changes(
                prev_skylight.as_deref(),
                &result.skylight,
                crate::chunk::SKY_FULL,
            ) | super::light::cube_region_changes(
                prev_blocklight.as_deref(),
                &result.blocklight,
                0,
            )
        };
        PredictionLightResult {
            result,
            mask,
            first_bake,
        }
    })?;

    // Build only the meshes something actually sampled: the edited cells'
    // geometry samplers, plus every section a changed light region reaches
    // through the one-cell mesh pad. Unchanged-light candidates drop here.
    let mut needed: FxHashSet<SectionPos> = always_mesh.into_iter().collect();
    let mut baked: FxHashMap<SectionPos, (Arc<[u8]>, Arc<[u8]>)> = FxHashMap::default();
    for light in &light_results {
        if light.mask == 0 {
            continue;
        }
        let pos = light.result.pos;
        baked.insert(
            pos,
            (
                Arc::clone(&light.result.skylight),
                Arc::clone(&light.result.blocklight),
            ),
        );
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if light.mask & super::light::region_bit(dx, dy, dz) != 0 {
                        needed.insert(SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz));
                    }
                }
            }
        }
    }
    // Patch the fresh cubes into the surviving mesh snapshots on the
    // coordinator (a few `Arc` swaps per job) so the builds stay independent.
    let meshes: Vec<PredictionMeshJob> = meshes
        .into_iter()
        .filter(|mesh| needed.contains(&mesh.pos()))
        .map(|mesh| match mesh {
            PredictionMeshJob::Build(mut job) => {
                for (&pos, (skylight, blocklight)) in &baked {
                    job.replace_light_snapshot(pos, Arc::clone(skylight), Arc::clone(blocklight));
                }
                PredictionMeshJob::Build(job)
            }
            remove => remove,
        })
        .collect();
    let mesh_results = run_parallel(pool, cancel, meshes, |mesh| match mesh {
        PredictionMeshJob::Build(job) => {
            let pos = job.pos;
            let revision = job.revision;
            let mesh = mesh_pool::build_inline(job)
                .expect("an uncancelled inline mesh build always yields a mesh");
            PredictionMeshResult::Built {
                pos,
                revision,
                mesh,
            }
        }
        PredictionMeshJob::Remove { pos, revision } => {
            PredictionMeshResult::Remove { pos, revision }
        }
    })?;
    Some(PredictionTerrainResult {
        guards,
        lights: light_results,
        meshes: mesh_results,
    })
}

struct BatchState<J, R> {
    next: usize,
    claimed: usize,
    completed: usize,
    failed: bool,
    jobs: Vec<Option<J>>,
    results: Vec<Option<R>>,
}

/// Run independent jobs with the caller participating: one pool helper task
/// per item (top priority, capped) plus the caller itself claim items off a
/// shared list; whoever claims an item runs it. Deadlock-free by construction:
/// on a saturated pool the caller simply processes every item itself. Returns
/// `None` when `cancel` fired or an item panicked (its helper counts the item
/// completed-but-failed, so the coordinator never waits forever).
fn run_parallel<J, R, F>(
    pool: &Arc<JobPool>,
    cancel: &JobCancel,
    jobs: Vec<J>,
    f: F,
) -> Option<Vec<R>>
where
    J: Send + 'static,
    R: Send + 'static,
    F: Fn(J) -> R + Send + Sync + 'static,
{
    let n = jobs.len();
    if n == 0 {
        return Some(Vec::new());
    }
    if cancel.is_cancelled() {
        return None;
    }
    if n == 1 {
        let job = jobs.into_iter().next().expect("n == 1");
        return Some(vec![f(job)]);
    }

    let shared = Arc::new((
        Mutex::new(BatchState {
            next: 0,
            claimed: 0,
            completed: 0,
            failed: false,
            jobs: jobs.into_iter().map(Some).collect(),
            results: (0..n).map(|_| None).collect(),
        }),
        Condvar::new(),
    ));
    let f = Arc::new(f);
    for _ in 0..(n - 1).min(MAX_BATCH_HELPERS) {
        let shared = Arc::clone(&shared);
        let f = Arc::clone(&f);
        let cancel = cancel.clone();
        pool.submit(PREDICTION_TERRAIN_PRIORITY, move || {
            batch_worker(&shared, &cancel, &*f);
        });
    }
    batch_worker(&shared, cancel, &*f);

    let (state, done) = &*shared;
    let mut s = state.lock().expect("batch state never poisoned");
    loop {
        if s.completed == n || (s.completed == s.claimed && cancel.is_cancelled()) {
            break;
        }
        // The timeout only exists to observe a cancel raced in by another
        // thread; completions always notify.
        let (guard, _) = done
            .wait_timeout(s, std::time::Duration::from_micros(500))
            .expect("batch state never poisoned");
        s = guard;
    }
    if s.completed < n || s.failed {
        return None;
    }
    Some(
        s.results
            .iter_mut()
            .map(|slot| slot.take().expect("all items completed"))
            .collect(),
    )
}

fn batch_worker<J, R>(
    shared: &(Mutex<BatchState<J, R>>, Condvar),
    cancel: &JobCancel,
    f: &(impl Fn(J) -> R + Sync),
) {
    let (state, done) = shared;
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let (i, job) = {
            let mut s = state.lock().expect("batch state never poisoned");
            if s.next >= s.jobs.len() {
                return;
            }
            let i = s.next;
            s.next += 1;
            s.claimed += 1;
            (i, s.jobs[i].take().expect("each index is claimed once"))
        };
        // A panicking item (an engine bug in a bake/build) must still count as
        // completed, or the coordinator would wait for it forever.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(job))).ok();
        let mut s = state.lock().expect("batch state never poisoned");
        s.failed |= result.is_none();
        s.results[i] = result;
        s.completed += 1;
        done.notify_all();
    }
}
