use rustc_hash::FxHashMap;
use std::sync::Arc;

use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::column::Column;
use petramond_world::light::LightRgb;
use petramond_world::section::Section;

use petramond_world::world::light::bake::{bake_section, LightBakeOutput, SectionBakeJob};

pub struct LightBakeQueue {
    backend: Backend,
    pending: FxHashMap<SectionPos, PendingLightBake>,
    next_id: u64,
}

#[derive(Clone)]
struct PendingLightBake {
    id: u64,
    cancel: crate::worker::JobCancel,
}

/// One queued per-section bake: the world crate's [`SectionBakeJob`] tagged
/// with the queue id its result must match.
pub struct LightBakeJob {
    id: u64,
    bake: SectionBakeJob,
}

pub struct LightBakeResult {
    id: u64,
    pub pos: SectionPos,
    pub revision: u64,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[LightRgb]>,
}

impl LightBakeResult {
    fn from_output(id: u64, out: LightBakeOutput) -> Self {
        Self {
            id,
            pos: out.pos,
            revision: out.revision,
            skylight: out.skylight,
            blocklight: out.blocklight,
        }
    }

    /// Build a result outside the async queue (prediction batch clips).
    /// `id` is unused by the prediction install path.
    pub fn from_batch_output(out: LightBakeOutput) -> Self {
        Self::from_output(0, out)
    }
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
        let Some(job) =
            petramond_world::world::light::batch::snapshot_batch(base, &fresh, sections, columns)
        else {
            return;
        };
        // Pending slots only for members the snapshot actually carries — an
        // absent-section member would otherwise wedge a slot no result clears.
        let mut cancels: Vec<(SectionPos, u64, crate::worker::JobCancel)> = Vec::new();
        for pos in job.member_positions() {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            let cancel = crate::worker::JobCancel::new();
            self.pending.insert(
                pos,
                PendingLightBake {
                    id,
                    cancel: cancel.clone(),
                },
            );
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
    pub fn snapshot(
        id: u64,
        pos: SectionPos,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Column>,
    ) -> Option<Self> {
        let bake = SectionBakeJob::snapshot(pos, sections, columns)?;
        Some(Self { id, bake })
    }

    pub fn pos(&self) -> SectionPos {
        self.bake.pos()
    }
}

/// Run one queued bake (the world crate's [`bake_section`]) and tag the result.
pub fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
    LightBakeResult::from_output(job.id, bake_section(job.bake))
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
        mut job: petramond_world::world::light::batch::LightBatchJob,
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
            for out in petramond_world::world::light::batch::run_light_bake_batch(job) {
                let Some((_, id, _)) = cancels.iter().find(|(p, _, _)| *p == out.pos) else {
                    continue;
                };
                let _ = tx.send(LightBakeResult::from_output(*id, out));
            }
        });
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.rx_res.try_recv().ok()
    }
}
