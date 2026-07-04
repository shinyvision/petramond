use std::collections::HashMap;
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
    pending: HashMap<SectionPos, PendingLightBake>,
    next_id: u64,
}

#[derive(Copy, Clone, Debug)]
struct PendingLightBake {
    id: u64,
}

struct LightBakeJob {
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
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    /// `key` is the shared-pool distance priority (lower = sooner).
    pub fn request(
        &mut self,
        key: i64,
        pos: SectionPos,
        sections: &HashMap<SectionPos, Arc<Section>>,
        columns: &HashMap<ChunkPos, Column>,
    ) {
        if self.pending.contains_key(&pos) {
            return;
        }
        let Some(section) = sections.get(&pos) else {
            self.pending.remove(&pos);
            return;
        };
        if !section.light_dirty {
            self.pending.remove(&pos);
            return;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let revision = section.light_revision;

        let sky = skylight::plan(pos, columns);
        let emitters = neighborhood::collect_emitters(pos, sections);
        let nbhd = (matches!(sky, SkyPlan::Flood { .. }) || !emitters.is_empty())
            .then(|| neighborhood::gather(pos, sections));

        self.pending.insert(pos, PendingLightBake { id });
        self.backend.submit(
            key,
            LightBakeJob {
                id,
                pos,
                revision,
                sky,
                nbhd,
                emitters,
            },
        );
    }

    pub fn cancel(&mut self, pos: SectionPos) {
        self.pending.remove(&pos);
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

fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
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
            .map(|n| ShapeStateSnapshot::from_sparse(n.states()))
            .unwrap_or_default();

        let skylight = match sky {
            SkyPlan::Full => vec![crate::chunk::SKY_FULL; SECTION_VOLUME].into(),
            SkyPlan::Dark => vec![0u8; SECTION_VOLUME].into(),
            SkyPlan::Flood { surface } => {
                let blocks =
                    blocks.expect("a flooding skylight bake carries its neighbourhood blocks");
                flood::skylight(pos, LightCells::new(blocks, &states), &surface, flood)
            }
        };

        let blocklight = if emitters.is_empty() {
            vec![0u8; SECTION_VOLUME].into()
        } else {
            let blocks = blocks.expect("a block-light bake carries its neighbourhood blocks");
            flood::block_light(pos, LightCells::new(blocks, &states), &emitters, flood)
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

    fn submit(&self, key: i64, job: LightBakeJob) {
        let tx = self.tx_res.clone();
        self.pool.submit(key, move || {
            let _ = tx.send(run_light_bake(job));
        });
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.rx_res.try_recv().ok()
    }
}
