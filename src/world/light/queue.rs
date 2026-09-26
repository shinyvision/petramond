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
    /// Events a multi-section report expanded into, handed out one per
    /// [`try_recv`](LightBakeQueue::try_recv).
    ready: Vec<LightBakeEvent>,
}

#[derive(Clone)]
struct PendingLightBake {
    id: u64,
    /// The section's `light_revision` when the bake was requested — what a
    /// failure report is judged against.
    revision: u64,
    cancel: crate::worker::JobCancel,
}

/// What the light stage reports for one requested section.
pub enum LightBakeEvent {
    Baked(LightBakeResult),
    /// The bake job panicked. Its pending slot is already released; the
    /// installer decides how the section's light settles (see
    /// `World::drain_light_bakes`).
    Failed { pos: SectionPos, revision: u64 },
}

/// One pool job's report on the backend channel. Exactly one per job (the
/// pool's stage contract): the baked members, or every member it was asked
/// for as failed.
enum BakeReport {
    Baked(Vec<LightBakeResult>),
    Failed(Vec<(SectionPos, u64)>),
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
            ready: Vec::new(),
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
        let revision = sections.get(&pos).map_or(0, |s| s.light_revision);
        let cancel = self.backend.submit(key, job);
        self.pending.insert(
            pos,
            PendingLightBake {
                id,
                revision,
                cancel,
            },
        );
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
                    revision: sections.get(&pos).map_or(0, |s| s.light_revision),
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

    /// The next finished bake or failure for a section whose request is
    /// still current (a cancelled or superseded request's report is
    /// dropped). A report always releases its pending slot, so a panicking
    /// bake can never leave the section dedup-blocked forever.
    pub fn try_recv(&mut self) -> Option<LightBakeEvent> {
        loop {
            if let Some(event) = self.ready.pop() {
                return Some(event);
            }
            match self.backend.try_recv()? {
                BakeReport::Baked(results) => {
                    for res in results {
                        if self.take_pending(res.pos, res.id).is_some() {
                            self.ready.push(LightBakeEvent::Baked(res));
                        }
                    }
                }
                BakeReport::Failed(members) => {
                    for (pos, id) in members {
                        if let Some(pending) = self.take_pending(pos, id) {
                            self.ready.push(LightBakeEvent::Failed {
                                pos,
                                revision: pending.revision,
                            });
                        }
                    }
                }
            }
            // Hand events out in report order.
            self.ready.reverse();
        }
    }

    fn take_pending(&mut self, pos: SectionPos, id: u64) -> Option<PendingLightBake> {
        if self.pending.get(&pos).is_some_and(|p| p.id == id) {
            self.pending.remove(&pos)
        } else {
            None
        }
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
    tx_res: std::sync::mpsc::Sender<BakeReport>,
    rx_res: std::sync::mpsc::Receiver<BakeReport>,
}

impl Backend {
    fn new(pool: std::sync::Arc<crate::worker::JobPool>) -> Self {
        let (tx_res, rx_res) = std::sync::mpsc::channel::<BakeReport>();
        Self {
            pool,
            tx_res,
            rx_res,
        }
    }

    fn submit(&self, key: i64, job: LightBakeJob) -> crate::worker::JobCancel {
        let pos = job.pos();
        let id = job.id;
        self.submit_run(key, pos, id, move || run_light_bake(job))
    }

    /// Queue one single-section bake under the stage contract: the job
    /// reports its result, nothing when cancelled, or — if `run` panics —
    /// [`BakeReport::Failed`] through its report slot.
    fn submit_run(
        &self,
        key: i64,
        pos: SectionPos,
        id: u64,
        run: impl FnOnce() -> LightBakeResult + Send + 'static,
    ) -> crate::worker::JobCancel {
        let cancel = crate::worker::JobCancel::new();
        let job_cancel = cancel.clone();
        let slot = crate::worker::ReportSlot::new(
            self.tx_res.clone(),
            BakeReport::Failed(vec![(pos, id)]),
            "light",
            pos,
        );
        self.pool.submit(key, move || {
            if job_cancel.is_cancelled() {
                slot.complete(BakeReport::Baked(Vec::new()));
                return;
            }
            slot.complete(BakeReport::Baked(vec![run()]));
        });
        cancel
    }

    /// One pool job bakes the whole batch and reports one [`LightBakeResult`]
    /// per surviving member, so the pump's freshness/stale handling is
    /// identical to per-section bakes. A panic fails every member.
    fn submit_batch(
        &self,
        key: i64,
        mut job: petramond_world::world::light::batch::LightBatchJob,
        cancels: Vec<(SectionPos, u64, crate::worker::JobCancel)>,
    ) {
        let Some(&(at, _, _)) = cancels.first() else {
            return;
        };
        let failed = BakeReport::Failed(cancels.iter().map(|(p, id, _)| (*p, *id)).collect());
        let slot = crate::worker::ReportSlot::new(self.tx_res.clone(), failed, "light batch", at);
        self.pool.submit(key, move || {
            job.retain_members(|pos| {
                cancels
                    .iter()
                    .any(|(p, _, c)| *p == pos && !c.is_cancelled())
            });
            if job.is_empty() {
                slot.complete(BakeReport::Baked(Vec::new()));
                return;
            }
            let results = petramond_world::world::light::batch::run_light_bake_batch(job)
                .into_iter()
                .filter_map(|out| {
                    let (_, id, _) = cancels.iter().find(|(p, _, _)| *p == out.pos)?;
                    Some(LightBakeResult::from_output(*id, out))
                })
                .collect();
            slot.complete(BakeReport::Baked(results));
        });
    }

    fn try_recv(&mut self) -> Option<BakeReport> {
        self.rx_res.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_bake_releases_its_pending_slot_and_reports_failed() {
        let pool = std::sync::Arc::new(crate::worker::JobPool::inline());
        let mut queue = LightBakeQueue::new(pool);
        let pos = SectionPos::new(0, 2, 0);
        let id = 41;
        let cancel = queue
            .backend
            .submit_run(0, pos, id, || panic!("injected light panic"));
        queue.pending.insert(
            pos,
            PendingLightBake {
                id,
                revision: 7,
                cancel,
            },
        );
        match queue.try_recv() {
            Some(LightBakeEvent::Failed { pos: p, revision }) => {
                assert_eq!((p, revision), (pos, 7));
            }
            _ => panic!("the failure must be reported"),
        }
        assert!(!queue.has_pending(), "the pending slot was released");
        assert!(queue.try_recv().is_none());
    }

    #[test]
    fn a_superseded_failure_is_dropped() {
        let pool = std::sync::Arc::new(crate::worker::JobPool::inline());
        let mut queue = LightBakeQueue::new(pool);
        let pos = SectionPos::new(1, 2, 3);
        queue.backend.submit_run(0, pos, 5, || panic!("injected light panic"));
        // A newer request (id 6) owns the slot now.
        queue.pending.insert(
            pos,
            PendingLightBake {
                id: 6,
                revision: 0,
                cancel: crate::worker::JobCancel::new(),
            },
        );
        assert!(queue.try_recv().is_none());
        assert!(queue.has_pending());
    }
}
