use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkPos};
use crate::mathh::IVec3;
use crate::mesh::{compute_chunk_blocklight_with_neighbors, compute_chunk_skylight_with_neighbors};

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
    /// World positions of light emitters (torches) in the center + halo chunks,
    /// gathered from their torch maps so the block-light flood needn't scan every
    /// block. Empty for the common torch-free neighbourhood.
    emitters: Vec<IVec3>,
}

pub(super) struct LightBakeResult {
    id: u64,
    pub pos: ChunkPos,
    pub revision: u64,
    pub band: Box<[u8]>,
    pub ylo: i32,
    pub yhi: i32,
    /// Block-light band + its own `[ylo, yhi]` (empty when no emitters are near).
    pub block_band: Box<[u8]>,
    pub block_ylo: i32,
    pub block_yhi: i32,
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
            emitters: collect_emitters(pos, chunks),
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

/// World positions of every light emitter in the bake neighbourhood (center + 8
/// neighbours), read cheaply from each chunk's sparse block-entity maps: every
/// torch, plus every furnace that is currently LIT (a furnace's lit state lives in
/// its entity, not its block id, so only the main thread — which holds the real
/// chunks — can tell which ones glow). Empty when nothing nearby emits, which lets
/// the block-light flood early-out.
fn collect_emitters(pos: ChunkPos, chunks: &HashMap<ChunkPos, Chunk>) -> Vec<IVec3> {
    let mut out = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let Some(c) = chunks.get(&ChunkPos::new(pos.cx + dx, pos.cz + dz)) else {
                continue;
            };
            let (ox, oz) = c.chunk_origin_world();
            // Invert a local block index (idx = y*256 + z*16 + x) to a world pos.
            let world_of = |key: u16| {
                IVec3::new(
                    ox + (key & 0x0F) as i32,
                    (key >> 8) as i32,
                    oz + ((key >> 4) & 0x0F) as i32,
                )
            };
            out.extend(c.torches().keys().map(|&k| world_of(k)));
            out.extend(
                c.furnaces()
                    .iter()
                    .filter(|(_, f)| f.is_lit())
                    .map(|(&k, _)| world_of(k)),
            );
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
        emitters,
    } = job;
    let (band, ylo, yhi) = compute_chunk_skylight_with_neighbors(&chunk, |cx, cz| {
        neighbours.get(&ChunkPos::new(cx, cz))
    });
    let (block_band, block_ylo, block_yhi) = compute_chunk_blocklight_with_neighbors(
        &chunk,
        |cx, cz| neighbours.get(&ChunkPos::new(cx, cz)),
        &emitters,
    );
    LightBakeResult {
        id,
        pos,
        revision,
        band,
        ylo,
        yhi,
        block_band,
        block_ylo,
        block_yhi,
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

        // A light bake is a pure function of its chunk+neighbour snapshot (see
        // `run_light_bake`), so it parallelises exactly like chunk gen. A SINGLE
        // worker was the world-load wall: every chunk's mesh waits for its 3x3
        // light band to bake (see `request_light_dependencies`), so at a large
        // render distance hundreds of chunks queued behind one core while the rest
        // sat idle. Use a pool, sized like the gen pool (reserve ~2 cores for the
        // main/render thread and the mesh rayon pool). Results carry an id +
        // revision and are matched on the main thread, so out-of-order completion
        // across workers is already handled.
        let rx_req = std::sync::Arc::new(std::sync::Mutex::new(rx_req));
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .saturating_sub(2)
            .max(2);
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let rx_req = rx_req.clone();
            let tx_res = tx_res.clone();
            let h = std::thread::Builder::new()
                .name("llamacraft-light".to_string())
                .spawn(move || loop {
                    // Hold the receiver lock only across the brief recv(); the bake
                    // itself runs unlocked, so workers pull jobs concurrently.
                    let job = {
                        let g = rx_req.lock().unwrap();
                        g.recv()
                    };
                    match job {
                        Ok(job) => {
                            if tx_res.send(run_light_bake(job)).is_err() {
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
