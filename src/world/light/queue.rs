use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_VOLUME};
use crate::column::Column;
use crate::mathh::IVec3;
use crate::section::Section;

use super::shape::{LightCells, ShapeStateSnapshot};
use super::skylight::SkyPlan;
use super::{flood, neighborhood, skylight};

pub(in crate::world) struct LightBakeQueue {
    backend: Backend,
    pending: FxHashMap<SectionPos, PendingLightBake>,
    next_id: u64,
}

#[derive(Clone)]
struct PendingLightBake {
    id: u64,
    cancel: crate::worker::JobCancel,
}

pub(in crate::world) struct LightBakeJob {
    id: u64,
    pos: SectionPos,
    revision: u64,
    sky: SkyPlan,
    nbhd: Option<neighborhood::Snapshot>,
    emitters: Vec<IVec3>,
}

pub(in crate::world) struct LightBakeResult {
    id: u64,
    pub pos: SectionPos,
    pub revision: u64,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[u8]>,
}

impl LightBakeQueue {
    pub fn new(pool: std::sync::Arc<crate::worker::JobPool>) -> Self {
        Self {
            backend: Backend::new(pool),
            pending: FxHashMap::default(),
            next_id: 1,
        }
    }

    /// `key` is the shared-pool distance priority (lower = sooner).
    pub fn request(
        &mut self,
        key: i64,
        pos: SectionPos,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Column>,
    ) {
        if self.pending.contains_key(&pos) {
            return;
        }
        let Some(job) = LightBakeJob::snapshot(self.next_id, pos, sections, columns) else {
            self.pending.remove(&pos);
            return;
        };

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let cancel = self.backend.submit(key, job);
        self.pending.insert(pos, PendingLightBake { id, cancel });
    }

    /// Request one 2×2×2 batch bake (streaming first-bakes: one shared 64³ flood,
    /// see `light::batch`). Members already pending are skipped; every member gets
    /// its own pending slot and cancel token, so cancelling one section only drops
    /// that member from the batch instead of killing its siblings' bakes.
    pub fn request_batch(
        &mut self,
        key: i64,
        base: SectionPos,
        members: &[SectionPos],
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Column>,
    ) {
        let fresh: Vec<SectionPos> = members
            .iter()
            .copied()
            .filter(|p| !self.pending.contains_key(p))
            .collect();
        if fresh.is_empty() {
            return;
        }
        let Some(job) = super::batch::snapshot_batch(base, &fresh, sections, columns) else {
            return;
        };
        // Pending slots only for members the snapshot actually carries — an
        // absent-section member would otherwise wedge a slot no result clears.
        let mut cancels: Vec<(SectionPos, u64, crate::worker::JobCancel)> = Vec::new();
        for pos in job.member_positions() {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            let cancel = crate::worker::JobCancel::new();
            self.pending.insert(pos, PendingLightBake {
                id,
                cancel: cancel.clone(),
            });
            cancels.push((pos, id, cancel));
        }
        self.backend.submit_batch(key, job, cancels);
    }

    pub fn cancel(&mut self, pos: SectionPos) {
        if let Some(pending) = self.pending.remove(&pos) {
            pending.cancel.cancel();
        }
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn try_recv(&mut self) -> Option<LightBakeResult> {
        while let Some(res) = self.backend.try_recv() {
            if self.pending.get(&res.pos).is_some_and(|p| p.id == res.id) {
                self.pending.remove(&res.pos);
                return Some(res);
            }
        }
        None
    }
}

impl LightBakeJob {
    /// Capture the same cheap section/column snapshot used by the ordinary
    /// asynchronous queue. Prediction bundles call this directly so their
    /// relight and mesh stages consume one post-edit snapshot.
    pub(in crate::world) fn snapshot(
        id: u64,
        pos: SectionPos,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Column>,
    ) -> Option<Self> {
        let section = sections.get(&pos)?;
        if !section.light_dirty {
            return None;
        }
        Self::snapshot_unchecked(id, pos, sections, columns)
    }

    /// [`Self::snapshot`] without the dirty gate — the batch parity test
    /// rebakes settled sections to compare against the batched bake.
    pub(in crate::world) fn snapshot_unchecked(
        id: u64,
        pos: SectionPos,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Column>,
    ) -> Option<Self> {
        let section = sections.get(&pos)?;
        let revision = section.light_revision;
        let sky = skylight::plan(pos, columns);
        let emitters = neighborhood::collect_emitters(pos, sections);
        let nbhd = (matches!(sky, SkyPlan::Flood { .. }) || !emitters.is_empty())
            .then(|| neighborhood::gather(pos, sections));
        Some(Self {
            id,
            pos,
            revision,
            sky,
            nbhd,
            emitters,
        })
    }

    pub(in crate::world) fn pos(&self) -> SectionPos {
        self.pos
    }
}

/// Per-light-thread reusable bake scratch: the assembled 48³ neighbourhood block
/// cube plus the flood working set. Streaming bakes run thousands of times across
/// several threads; reusing these keeps ~220 KB of per-bake churn off the allocator
/// (the returned per-section light cubes are still allocated fresh — they outlive
/// the bake).
struct BakeScratch {
    blocks: Box<[u8]>,
    flood: flood::FloodScratch,
}

thread_local! {
    static BAKE_SCRATCH: std::cell::RefCell<BakeScratch> = std::cell::RefCell::new(BakeScratch {
        blocks: vec![0u8; super::NBHD_VOLUME].into_boxed_slice(),
        flood: flood::FloodScratch::new(),
    });
}

/// Total worker nanoseconds and jobs spent on light bakes — temporary perf-session
/// diagnostics read by the out-of-tree streaming profiler.
pub(crate) static LIGHT_STAGE_NS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
pub(crate) static LIGHT_STAGE_JOBS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(in crate::world) fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
    let t_stage = std::time::Instant::now();
    let LightBakeJob {
        id,
        pos,
        revision,
        sky,
        nbhd,
        emitters,
    } = job;

    BAKE_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let BakeScratch { blocks, flood } = &mut *scratch;

        let blocks: Option<&[u8]> = nbhd.as_ref().map(|n| {
            neighborhood::assemble_blocks(n, blocks);
            &blocks[..]
        });
        let states = nbhd
            .as_ref()
            .map(|n| ShapeStateSnapshot::from_sparse(n.states(), super::NBHD_VOLUME))
            .unwrap_or_default();

        let skylight = match sky {
            SkyPlan::Full => vec![crate::chunk::SKY_FULL; SECTION_VOLUME].into(),
            SkyPlan::Dark => vec![0u8; SECTION_VOLUME].into(),
            SkyPlan::Flood { surface } => {
                let blocks =
                    blocks.expect("a flooding skylight bake carries its neighbourhood blocks");
                flood::skylight(
                    pos,
                    LightCells::new(blocks, &states, super::NBHD),
                    &surface,
                    flood,
                )
            }
        };

        let blocklight = if emitters.is_empty() {
            vec![0u8; SECTION_VOLUME].into()
        } else {
            let blocks = blocks.expect("a block-light bake carries its neighbourhood blocks");
            flood::block_light(
                pos,
                LightCells::new(blocks, &states, super::NBHD),
                &emitters,
                flood,
            )
        };

        LIGHT_STAGE_NS.fetch_add(
            t_stage.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        LIGHT_STAGE_JOBS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        LightBakeResult {
            id,
            pos,
            revision,
            skylight,
            blocklight,
        }
    })
}

/// Light-stage adapter over the shared [`crate::worker::JobPool`]: `submit` queues a
/// bake at a distance priority, `try_recv` drains finished cubes on the main thread.
struct Backend {
    pool: std::sync::Arc<crate::worker::JobPool>,
    tx_res: std::sync::mpsc::Sender<LightBakeResult>,
    rx_res: std::sync::mpsc::Receiver<LightBakeResult>,
}

impl Backend {
    fn new(pool: std::sync::Arc<crate::worker::JobPool>) -> Self {
        let (tx_res, rx_res) = std::sync::mpsc::channel::<LightBakeResult>();
        Self {
            pool,
            tx_res,
            rx_res,
        }
    }

    fn submit(&self, key: i64, job: LightBakeJob) -> crate::worker::JobCancel {
        let cancel = crate::worker::JobCancel::new();
        let job_cancel = cancel.clone();
        let tx = self.tx_res.clone();
        self.pool.submit(key, move || {
            if job_cancel.is_cancelled() {
                return;
            }
            let _ = tx.send(run_light_bake(job));
        });
        cancel
    }

    /// One pool job bakes the whole batch and emits one [`LightBakeResult`] per
    /// surviving member through the ordinary result channel, so the pump's
    /// freshness/stale handling is identical to per-section bakes.
    fn submit_batch(
        &self,
        key: i64,
        mut job: super::batch::LightBatchJob,
        cancels: Vec<(SectionPos, u64, crate::worker::JobCancel)>,
    ) {
        let tx = self.tx_res.clone();
        self.pool.submit(key, move || {
            job.retain_members(|pos| {
                cancels
                    .iter()
                    .any(|(p, _, c)| *p == pos && !c.is_cancelled())
            });
            if job.is_empty() {
                return;
            }
            for out in super::batch::run_light_bake_batch(job) {
                let Some((_, id, _)) = cancels.iter().find(|(p, _, _)| *p == out.pos) else {
                    continue;
                };
                let _ = tx.send(LightBakeResult {
                    id: *id,
                    pos: out.pos,
                    revision: out.revision,
                    skylight: out.skylight,
                    blocklight: out.blocklight,
                });
            }
        });
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.rx_res.try_recv().ok()
    }
}
