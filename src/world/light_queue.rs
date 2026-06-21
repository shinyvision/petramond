use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkPos};
#[cfg(not(target_arch = "wasm32"))]
use crate::mesh::compute_chunk_skylight_with_neighbors;

#[cfg(target_arch = "wasm32")]
const LIGHT_REQ_TAG: u8 = b'L';
#[cfg(target_arch = "wasm32")]
const LIGHT_RES_TAG: u8 = b'l';

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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
struct Backend {
    tx_req: std::sync::mpsc::Sender<LightBakeJob>,
    rx_res: std::sync::mpsc::Receiver<LightBakeResult>,
    _handle: std::thread::JoinHandle<()>,
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
struct Backend {
    worker: web_sys::Worker,
    ready: std::rc::Rc<std::cell::RefCell<Vec<LightBakeResult>>>,
}

#[cfg(target_arch = "wasm32")]
impl Backend {
    fn new() -> Self {
        use wasm_bindgen::prelude::Closure;
        use wasm_bindgen::JsCast;

        let opts = web_sys::WorkerOptions::new();
        opts.set_type(web_sys::WorkerType::Module);
        let worker =
            web_sys::Worker::new_with_options("worker_host.js", &opts).expect("spawn light worker");
        let ready = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let ready_cb = ready.clone();
        let onmsg =
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
                if let Some(buf) = ev.data().dyn_ref::<js_sys::ArrayBuffer>() {
                    let bytes = js_sys::Uint8Array::new(buf).to_vec();
                    if let Some(res) = decode_light_result(&bytes) {
                        ready_cb.borrow_mut().push(res);
                    }
                }
            });
        worker.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
        onmsg.forget();
        Self { worker, ready }
    }

    fn submit(&mut self, job: LightBakeJob) {
        let bytes = encode_light_job(job);
        let arr = js_sys::Uint8Array::from(&bytes[..]);
        let _ = self.worker.post_message(&arr);
    }

    fn try_recv(&mut self) -> Option<LightBakeResult> {
        self.ready.borrow_mut().pop()
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for Backend {
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

#[cfg(target_arch = "wasm32")]
fn encode_light_job(job: LightBakeJob) -> Vec<u8> {
    let mut neighbours: Vec<_> = job.neighbours.into_values().collect();
    neighbours.sort_by_key(|c| (c.cz, c.cx));
    let per_chunk = 8
        + crate::chunk::CHUNK_SX * crate::chunk::CHUNK_SZ * std::mem::size_of::<u16>()
        + crate::chunk::VOLUME;
    let mut out = Vec::with_capacity(1 + 8 + 8 + 8 + 1 + per_chunk * (1 + neighbours.len()));
    out.push(LIGHT_REQ_TAG);
    out.extend_from_slice(&job.id.to_le_bytes());
    out.extend_from_slice(&job.pos.cx.to_le_bytes());
    out.extend_from_slice(&job.pos.cz.to_le_bytes());
    out.extend_from_slice(&job.revision.to_le_bytes());
    out.push(neighbours.len().min(u8::MAX as usize) as u8);
    encode_light_chunk(&mut out, &job.chunk);
    for chunk in neighbours.into_iter().take(u8::MAX as usize) {
        encode_light_chunk(&mut out, &chunk);
    }
    out
}

#[cfg(target_arch = "wasm32")]
fn encode_light_chunk(out: &mut Vec<u8>, chunk: &Chunk) {
    out.extend_from_slice(&chunk.cx.to_le_bytes());
    out.extend_from_slice(&chunk.cz.to_le_bytes());
    for &h in chunk.heightmap.iter() {
        out.extend_from_slice(&h.to_le_bytes());
    }
    out.extend_from_slice(chunk.blocks_slice());
}

#[cfg(target_arch = "wasm32")]
fn decode_light_result(bytes: &[u8]) -> Option<LightBakeResult> {
    if bytes.first().copied()? != LIGHT_RES_TAG {
        return None;
    }
    let mut off = 1;
    let id = read_u64(bytes, &mut off)?;
    let cx = read_i32(bytes, &mut off)?;
    let cz = read_i32(bytes, &mut off)?;
    let revision = read_u64(bytes, &mut off)?;
    let ylo = read_i32(bytes, &mut off)?;
    let yhi = read_i32(bytes, &mut off)?;
    let len = read_u32(bytes, &mut off)? as usize;
    let band = take(bytes, &mut off, len)?.to_vec().into_boxed_slice();
    Some(LightBakeResult {
        id,
        pos: ChunkPos::new(cx, cz),
        revision,
        band,
        ylo,
        yhi,
    })
}

#[cfg(target_arch = "wasm32")]
fn read_i32(bytes: &[u8], off: &mut usize) -> Option<i32> {
    Some(i32::from_le_bytes(take_array(bytes, off)?))
}

#[cfg(target_arch = "wasm32")]
fn read_u32(bytes: &[u8], off: &mut usize) -> Option<u32> {
    Some(u32::from_le_bytes(take_array(bytes, off)?))
}

#[cfg(target_arch = "wasm32")]
fn read_u64(bytes: &[u8], off: &mut usize) -> Option<u64> {
    Some(u64::from_le_bytes(take_array(bytes, off)?))
}

#[cfg(target_arch = "wasm32")]
fn take_array<const N: usize>(bytes: &[u8], off: &mut usize) -> Option<[u8; N]> {
    let src = take(bytes, off, N)?;
    src.try_into().ok()
}

#[cfg(target_arch = "wasm32")]
fn take<'a>(bytes: &'a [u8], off: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = off.checked_add(len)?;
    let out = bytes.get(*off..end)?;
    *off = end;
    Some(out)
}
