//! Worker abstraction: off-thread chunk generation.
//!
//! A thread pool running reused `ChunkGenerator`s. The world submits requests
//! and drains results: `submit` requests, `try_recv` drains.

use crate::chunk::Chunk;
use crate::worldgen::{driver::ChunkGenerator, generate_chunk_with};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

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
// Thread pool using std::sync::mpsc + persistent worker threads.
// ---------------------------------------------------------------------------
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
        let mut handles = Vec::with_capacity(n);
        for _ in 0..n {
            let rx_req = rx_req.clone();
            let tx_res = tx_res.clone();
            let mut generator_seed = seed;
            let mut generator = ChunkGenerator::new(generator_seed);
            let h = thread::spawn(move || loop {
                let req = {
                    let g = rx_req.lock().unwrap();
                    g.recv()
                };
                match req {
                    Ok(r) => {
                        if r.seed != generator_seed {
                            generator_seed = r.seed;
                            generator = ChunkGenerator::new(generator_seed);
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
