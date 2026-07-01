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
    pub fn new() -> Self {
        Self {
            backend: Backend::new(),
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn request(
        &mut self,
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
        self.backend.submit(LightBakeJob {
            id,
            pos,
            revision,
            sky,
            nbhd,
            emitters,
        });
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

fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
    let LightBakeJob {
        id,
        pos,
        revision,
        sky,
        nbhd,
        emitters,
    } = job;

    let blocks = nbhd.as_ref().map(neighborhood::assemble_blocks);
    let states = nbhd
        .as_ref()
        .map(|n| ShapeStateSnapshot::from_sparse(n.states()))
        .unwrap_or_default();

    let skylight = match sky {
        SkyPlan::Full => vec![crate::chunk::SKY_FULL; SECTION_VOLUME].into(),
        SkyPlan::Dark => vec![0u8; SECTION_VOLUME].into(),
        SkyPlan::Flood { surface } => {
            let blocks = blocks
                .as_deref()
                .expect("a flooding skylight bake carries its neighbourhood blocks");
            flood::skylight(pos, LightCells::new(blocks, &states), &surface)
        }
    };

    let blocklight = if emitters.is_empty() {
        vec![0u8; SECTION_VOLUME].into()
    } else {
        let blocks = blocks
            .as_deref()
            .expect("a block-light bake carries its neighbourhood blocks");
        flood::block_light(pos, LightCells::new(blocks, &states), &emitters)
    };

    LightBakeResult {
        id,
        pos,
        revision,
        skylight,
        blocklight,
    }
}

struct Backend {
    tx_req: std::sync::mpsc::Sender<LightBakeJob>,
    rx_res: std::sync::mpsc::Receiver<LightBakeResult>,
    _handles: Vec<std::thread::JoinHandle<()>>,
}

impl Backend {
    fn new() -> Self {
        let (tx_req, rx_req) = std::sync::mpsc::channel::<LightBakeJob>();
        let (tx_res, rx_res) = std::sync::mpsc::channel::<LightBakeResult>();

        let rx_req = std::sync::Arc::new(std::sync::Mutex::new(rx_req));
        let (_, n, _) = crate::worker::background_thread_counts();
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let rx_req = rx_req.clone();
            let tx_res = tx_res.clone();
            let h = std::thread::Builder::new()
                .name("llamacraft-light".to_string())
                .spawn(move || loop {
                    let job = {
                        let g = rx_req.lock().unwrap();
                        g.recv()
                    };
                    match job {
                        Ok(job) => {
                            let res = run_light_bake(job);
                            if tx_res.send(res).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                })
                .expect("spawn light worker");
            handles.push(h);
        }
        Self {
            tx_req,
            rx_res,
            _handles: handles,
        }
    }

    fn submit(&self, job: LightBakeJob) {
        let _ = self.tx_req.send(job);
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.rx_res.try_recv().ok()
    }
}
