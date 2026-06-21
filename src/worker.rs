//! Worker abstraction: off-thread chunk generation.
//!
//! Native: thread pool running reused `ChunkGenerator`s.
//! Web: dedicated Worker whose source is `src/bin/worker_wasm.rs` built
//! with `--target wasm32-unknown-unknown` and loaded as a module worker.
//!
//! The API is identical from the world's POV: `submit` requests, `try_recv`
//! drains results.

use crate::chunk::Chunk;
#[cfg(not(target_arch = "wasm32"))]
use crate::worldgen::{
    classic::terrain::NoiseCache, driver::ChunkGenerator, generate_chunk_with,
};

#[derive(Copy, Clone, Debug)]
pub struct GenRequest {
    pub cx: i32,
    pub cz: i32,
    pub seed: u32,
}

pub struct GenResult {
    pub cx: i32,
    pub cz: i32,
    pub chunk: Chunk,
}

// ---------------------------------------------------------------------------
// Native: thread pool using std::sync::mpsc + scoped threads.
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
pub use native_impl::*;

#[cfg(not(target_arch = "wasm32"))]
mod native_impl {
    use super::*;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread;

    pub struct WorkerPool {
        tx_req: Sender<GenRequest>,
        rx_res: Mutex<Receiver<GenResult>>,
        _handles: Vec<thread::JoinHandle<()>>,
    }

    impl WorkerPool {
        pub fn new(seed: u32) -> Self {
            let (tx_req, rx_req) = channel::<GenRequest>();
            let (tx_res, rx_res) = channel::<GenResult>();
            // Use almost all cores for chunk gen (it is ~80% of CPU and the
            // bottleneck for world streaming), reserving ~2 for the main/render
            // thread and the mesh rayon pool. The single Mutex<Receiver> is held
            // only across the brief recv(); gen runs unlocked, so contention is
            // negligible even at high thread counts.
            let n = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .saturating_sub(2)
                .max(2);
            let rx_req = Arc::new(Mutex::new(rx_req));
            // One cache shared by every worker: the disc-fill burst submits many
            // adjacent chunks whose 32×32 regions overlap, so pooling the per-column
            // noise across threads samples each lattice column ~once, not ~5×.
            let shared_cache = Arc::new(NoiseCache::new());
            let mut handles = Vec::with_capacity(n);
            for _ in 0..n {
                let rx_req = rx_req.clone();
                let tx_res = tx_res.clone();
                let cache = shared_cache.clone();
                let mut generator_seed = seed;
                let mut generator = ChunkGenerator::with_cache(generator_seed, cache.clone());
                let h = thread::spawn(move || loop {
                    let req = {
                        let g = rx_req.lock().unwrap();
                        g.recv()
                    };
                    match req {
                        Ok(r) => {
                            if r.seed != generator_seed {
                                generator_seed = r.seed;
                                generator = ChunkGenerator::with_cache(generator_seed, cache.clone());
                            }
                            let chunk = generate_chunk_with(&generator, r.cx, r.cz);
                            if tx_res
                                .send(GenResult {
                                    cx: r.cx,
                                    cz: r.cz,
                                    chunk,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                });
                handles.push(h);
            }
            Self {
                tx_req,
                rx_res: Mutex::new(rx_res),
                _handles: handles,
            }
        }

        pub fn submit(&self, req: GenRequest) {
            let _ = self.tx_req.send(req);
        }
        pub fn try_recv(&self) -> Option<GenResult> {
            self.rx_res.lock().unwrap().try_recv().ok()
        }
    }
}

// ---------------------------------------------------------------------------
// Web: Web Worker. Code is built separately + loaded at runtime; here we
// only orchestrate sending/receiving via JS bridge. See `src/bin/worker_wasm.rs`.
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
pub use web_impl::*;

#[cfg(target_arch = "wasm32")]
mod web_impl {
    use super::*;
    use js_sys::{ArrayBuffer, Uint8Array};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{MessageEvent, Worker};

    /// A request as serialized to the worker: 12 bytes (cx,cz,seed i32×3).
    pub struct WorkerPool {
        worker: Worker,
        /// Pending results queued (we receive via callback into this buffer).
        pending: std::rc::Rc<std::cell::RefCell<Vec<GenResult>>>,
    }

    impl WorkerPool {
        pub fn new(_seed: u32) -> Self {
            let worker = spawn_worker();
            let pending = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let p2 = pending.clone();
            let onmsg = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
                if let Some(buf) = ev.data().dyn_ref::<ArrayBuffer>() {
                    let bytes = Uint8Array::new(buf).to_vec();
                    if let Some(res) = decode_result(&bytes) {
                        p2.borrow_mut().push(res);
                    }
                }
            });
            worker.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));
            // Leak closure: worker lives for entire program.
            onmsg.forget();
            Self { worker, pending }
        }

        pub fn submit(&self, req: GenRequest) {
            let mut bytes = [0u8; 12];
            bytes[0..4].copy_from_slice(&req.cx.to_le_bytes());
            bytes[4..8].copy_from_slice(&req.cz.to_le_bytes());
            bytes[8..12].copy_from_slice(&req.seed.to_le_bytes());
            let arr = Uint8Array::from(&bytes[..]);
            let _ = self.worker.post_message(&arr);
        }

        pub fn try_recv(&self) -> Option<GenResult> {
            self.pending.borrow_mut().pop()
        }
    }

    fn spawn_worker() -> Worker {
        // Module worker: loads `worker_host.js` which imports the wasm-bindgen
        // worker module. Requires COOP/COEP for cross-origin isolation.
        let opts = web_sys::WorkerOptions::new();
        opts.set_type(web_sys::WorkerType::Module);
        Worker::new_with_options("worker_host.js", &opts).expect("spawn worker")
    }

    fn decode_result(bytes: &[u8]) -> Option<GenResult> {
        const BIOME_BYTES: usize = crate::chunk::CHUNK_SX * crate::chunk::CHUNK_SZ;
        if bytes.len() < 8 {
            return None;
        }
        let cx = i32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let cz = i32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let blocks = &bytes[8..];
        if blocks.len() != crate::chunk::VOLUME + BIOME_BYTES {
            return None;
        }
        let mut chunk = crate::chunk::Chunk::new(cx, cz);
        for (i, &b) in blocks[..crate::chunk::VOLUME].iter().enumerate() {
            // Direct write to avoid per-block dirty/heightmap update cost.
            chunk.blocks_slice_mut()[i] = b;
        }
        chunk
            .biomes_slice_mut()
            .copy_from_slice(&blocks[crate::chunk::VOLUME..crate::chunk::VOLUME + BIOME_BYTES]);
        // Rebuild heightmap + mark dirty.
        chunk.recompute_heightmap();
        chunk.dirty = true;
        Some(GenResult { cx, cz, chunk })
    }

    impl Drop for WorkerPool {
        fn drop(&mut self) {
            self.worker.terminate();
        }
    }
}
