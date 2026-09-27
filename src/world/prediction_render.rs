use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use crate::worker::{JobCancel, JobPool};
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::SectionPos;

use super::light::{
    run_light_bake, run_light_bake_batch, LightBakeJob, LightBakeResult, LightBatchJob,
};
use super::mesh_pool::{self, MeshJob};

const PREDICTION_TERRAIN_PRIORITY: i64 = i64::MIN;

const MAX_BATCH_HELPERS: usize = 16;

#[derive(Copy, Clone)]
pub(super) struct SectionGuard {
    pub pos: SectionPos,
    pub light_revision: u64,
    pub mesh_revision: u64,
}

pub(super) enum PredictionMeshJob {
    Build(Box<MeshJob>),
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
        mesh: Box<ChunkMesh>,
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

type LightCubes = (
    Option<Arc<[u8]>>,
    Option<Arc<[petramond_world::light::LightRgb]>>,
);

pub(super) struct PredictionLightJob {
    pub job: LightBakeJob,
    pub prev_skylight: Option<Arc<[u8]>>,
    pub prev_blocklight: Option<Arc<[petramond_world::light::LightRgb]>>,
}

pub(super) enum PredictionLightUnit {
    Single(Box<PredictionLightJob>),
    Batch {
        job: LightBatchJob,
        prev: Vec<LightCubes>,
    },
}

pub(super) struct PredictionLightResult {
    pub result: LightBakeResult,
    pub mask: u32,
    pub first_bake: bool,
}

pub(super) struct PredictionTerrainResult {
    pub guards: Vec<SectionGuard>,
    pub lights: Vec<PredictionLightResult>,
    pub meshes: Vec<PredictionMeshResult>,
}

pub(super) struct PredictionTerrainWork {
    pub guards: Vec<SectionGuard>,
    pub lights: Vec<PredictionLightUnit>,
    pub meshes: Vec<PredictionMeshJob>,
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
        let light_positions: Box<[SectionPos]> = work
            .lights
            .iter()
            .flat_map(|unit| match unit {
                PredictionLightUnit::Single(light) => vec![light.job.pos()],
                PredictionLightUnit::Batch { job, .. } => job.member_positions().collect(),
            })
            .collect();
        let mesh_positions: Box<[SectionPos]> =
            work.meshes.iter().map(PredictionMeshJob::pos).collect();
        let mut affected: Vec<SectionPos> = work.guards.iter().map(|guard| guard.pos).collect();
        for &pos in mesh_positions.iter() {
            if !affected.contains(&pos) {
                affected.push(pos);
            }
        }

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

    pub(super) fn pool(&self) -> &Arc<JobPool> {
        &self.pool
    }
}

pub(super) fn run_prediction_terrain_synchronously(
    work: PredictionTerrainWork,
    pool: &Arc<JobPool>,
) -> Option<PredictionTerrainResult> {
    run_prediction_terrain(work, &JobCancel::new(), pool)
}

type BakedLight = (Arc<[u8]>, Arc<[petramond_world::light::LightRgb]>);

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
    let light_batches = run_parallel(pool, cancel, lights, |unit| match unit {
        PredictionLightUnit::Single(light) => {
            let PredictionLightJob {
                job,
                prev_skylight,
                prev_blocklight,
            } = *light;
            vec![finish_prediction_light(
                run_light_bake(job),
                prev_skylight,
                prev_blocklight,
            )]
        }
        PredictionLightUnit::Batch { job, prev } => {
            let outs = run_light_bake_batch(job);
            debug_assert_eq!(outs.len(), prev.len());
            outs.into_iter()
                .zip(prev)
                .map(|(out, (prev_skylight, prev_blocklight))| {
                    finish_prediction_light(
                        LightBakeResult::from_batch_output(out),
                        prev_skylight,
                        prev_blocklight,
                    )
                })
                .collect()
        }
    })?;
    let light_results: Vec<PredictionLightResult> = light_batches.into_iter().flatten().collect();

    let mut needed: FxHashSet<SectionPos> = always_mesh.into_iter().collect();
    let mut baked: FxHashMap<SectionPos, BakedLight> = FxHashMap::default();
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
            let mesh = mesh_pool::build_inline(*job)
                .expect("an uncancelled inline mesh build always yields a mesh");
            PredictionMeshResult::Built {
                pos,
                revision,
                mesh: Box::new(mesh),
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

fn finish_prediction_light(
    result: LightBakeResult,
    prev_skylight: Option<Arc<[u8]>>,
    prev_blocklight: Option<Arc<[petramond_world::light::LightRgb]>>,
) -> PredictionLightResult {
    let first_bake = prev_skylight.is_none();
    let mask = if first_bake {
        super::light::REGION_ALL
    } else {
        super::light::cube_region_changes(
            prev_skylight.as_deref(),
            &result.skylight,
            petramond_world::chunk::SKY_FULL,
        ) | super::light::cube_region_changes(
            prev_blocklight.as_deref(),
            &result.blocklight,
            petramond_world::light::LightRgb::ZERO,
        )
    };
    PredictionLightResult {
        result,
        mask,
        first_bake,
    }
}

struct BatchState<J, R> {
    next: usize,
    claimed: usize,
    completed: usize,
    failed: bool,
    jobs: Vec<Option<J>>,
    results: Vec<Option<R>>,
}

/// The caller and one helper task per item claim work off a shared list; whoever grabs an item runs
/// it. On a saturated pool the caller just does all the work itself, so it can't deadlock. Returns
/// `None` if `cancel` fires or an item panics (its helper marks it completed-but-failed so we never
/// hang waiting).
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
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(job))).ok();
        let mut s = state.lock().expect("batch state never poisoned");
        s.failed |= result.is_none();
        s.results[i] = result;
        s.completed += 1;
        done.notify_all();
    }
}
