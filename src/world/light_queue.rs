use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkPos};
use crate::mesh::compute_chunk_skylight_with_neighbors;

pub(super) struct LightBakeQueue {
    backend: Backend,
    pending: HashMap<ChunkPos, PendingLightBake>,
    next_id: u64,
}

#[derive(Copy, Clone, Debug)]
struct PendingLightBake {
    id: u64,
}

struct LightBakeJob {
    id: u64,
    pos: ChunkPos,
    revision: u64,
    chunk: Chunk,
    neighbours: HashMap<ChunkPos, Chunk>,
}

pub(super) struct LightBakeResult {
    id: u64,
    pub pos: ChunkPos,
    pub revision: u64,
    pub band: Box<[u8]>,
    pub ylo: i32,
    pub yhi: i32,
}

impl LightBakeQueue {
    pub fn new() -> Self {
        Self {
            backend: Backend::new(),
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn request(&mut self, pos: ChunkPos, chunks: &HashMap<ChunkPos, Chunk>) {
        if self.pending.contains_key(&pos) {
            return;
        }

        let Some(chunk) = chunks.get(&pos) else {
            self.pending.remove(&pos);
            return;
        };
        if !chunk.light_dirty {
            self.pending.remove(&pos);
            return;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let revision = chunk.light_revision;
        let job = LightBakeJob {
            id,
            pos,
            revision,
            chunk: chunk.snapshot_for_light_bake(),
            neighbours: snapshot_neighbours(pos, chunks),
        };
        self.pending.insert(pos, PendingLightBake { id });
        self.backend.submit(job);
    }

    pub fn cancel(&mut self, pos: ChunkPos) {
        self.pending.remove(&pos);
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

fn snapshot_neighbours(
    pos: ChunkPos,
    chunks: &HashMap<ChunkPos, Chunk>,
) -> HashMap<ChunkPos, Chunk> {
    let mut out = HashMap::with_capacity(8);
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let p = ChunkPos::new(pos.cx + dx, pos.cz + dz);
            if let Some(chunk) = chunks.get(&p) {
                out.insert(p, chunk.snapshot_for_light_bake());
            }
        }
    }
    out
}

fn run_light_bake(job: LightBakeJob) -> LightBakeResult {
    let LightBakeJob {
        id,
        pos,
        revision,
        chunk,
        neighbours,
    } = job;
    let (band, ylo, yhi) = compute_chunk_skylight_with_neighbors(&chunk, |cx, cz| {
        neighbours.get(&ChunkPos::new(cx, cz))
    });
    LightBakeResult {
        id,
        pos,
        revision,
        band,
        ylo,
        yhi,
    }
}

struct Backend {
    tx_req: std::sync::mpsc::Sender<LightBakeJob>,
    rx_res: std::sync::mpsc::Receiver<LightBakeResult>,
    _handle: std::thread::JoinHandle<()>,
}

impl Backend {
    fn new() -> Self {
        let (tx_req, rx_req) = std::sync::mpsc::channel::<LightBakeJob>();
        let (tx_res, rx_res) = std::sync::mpsc::channel::<LightBakeResult>();
        let _handle = std::thread::Builder::new()
            .name("llamacraft-light".to_string())
            .spawn(move || {
                while let Ok(job) = rx_req.recv() {
                    if tx_res.send(run_light_bake(job)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn light worker");
        Self {
            tx_req,
            rx_res,
            _handle,
        }
    }

    fn submit(&self, job: LightBakeJob) {
        let _ = self.tx_req.send(job);
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.rx_res.try_recv().ok()
    }
}
