use rustc_hash::FxHashMap;
use std::sync::Arc;

use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::column::Column;
use petramond_world::light::LightRgb;

use petramond_world::world::light::bake::{bake_section, LightBakeOutput, SectionBakeJob};

pub struct LightBakeQueue {
    backend: Backend,
    pending: FxHashMap<SectionPos, PendingLightBake>,
    next_id: u64,
    ready: Vec<LightBakeEvent>,
    requested: u64,
    landed: u64,
}

#[derive(Clone)]
struct PendingLightBake {
    id: u64,
    revision: u64,
    cancel: crate::worker::JobCancel,
}

pub enum LightBakeEvent {
    Baked(LightBakeResult),
    Failed { pos: SectionPos, revision: u64 },
}

enum BakeReport {
    Baked(Vec<LightBakeResult>),
    Failed(Vec<(SectionPos, u64)>),
}

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
            requested: 0,
            landed: 0,
        }
    }

    /// Bakes submitted and results accepted so far (operation counts for instruments).
    pub fn stats(&self) -> (u64, u64) {
        (self.requested, self.landed)
    }

    pub fn request(
        &mut self,
        key: i64,
        pos: SectionPos,
        sections: &petramond_world::world::section_map::SectionMap,
        columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
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
        self.requested += 1;
        self.pending.insert(
            pos,
            PendingLightBake {
                id,
                revision,
                cancel,
            },
        );
    }

    pub fn request_batch(
        &mut self,
        key: i64,
        base: SectionPos,
        members: &[SectionPos],
        sections: &petramond_world::world::section_map::SectionMap,
        columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
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
        self.requested += cancels.len() as u64;
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

    pub fn try_recv(&mut self) -> Option<LightBakeEvent> {
        loop {
            if let Some(event) = self.ready.pop() {
                return Some(event);
            }
            match self.backend.try_recv()? {
                BakeReport::Baked(results) => {
                    for res in results {
                        if self.take_pending(res.pos, res.id).is_some() {
                            self.landed += 1;
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
    pub fn snapshot(
        id: u64,
        pos: SectionPos,
        sections: &petramond_world::world::section_map::SectionMap,
        columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
    ) -> Option<Self> {
        let bake = SectionBakeJob::snapshot(pos, sections, columns)?;
        Some(Self { id, bake })
    }

    pub fn pos(&self) -> SectionPos {
        self.bake.pos()
    }
}

pub fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
    LightBakeResult::from_output(job.id, bake_section(job.bake))
}

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
        queue
            .backend
            .submit_run(0, pos, 5, || panic!("injected light panic"));
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
