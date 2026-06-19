//! Web Worker entry: receives `GenRequest {cx,cz,seed}` (12 bytes LE)
//! via `postMessage`, runs chunk generation, replies with 8 + VOLUME bytes.

use std::cell::RefCell;

use llamacraft::chunk::VOLUME;
use llamacraft::worldgen::{driver::ChunkGenerator, generate_chunk_with};
use wasm_bindgen::prelude::*;

thread_local! {
    static GENERATOR: RefCell<Option<(u32, ChunkGenerator)>> = RefCell::new(None);
}

#[wasm_bindgen]
pub fn worker_entry(payload: &[u8]) -> Vec<u8> {
    console_error_panic_hook::set_once();
    if payload.len() < 12 {
        return Vec::new();
    }
    let cx = i32::from_le_bytes(payload[0..4].try_into().unwrap());
    let cz = i32::from_le_bytes(payload[4..8].try_into().unwrap());
    let seed = u32::from_le_bytes(payload[8..12].try_into().unwrap());
    let chunk = generate_cached(seed, cx, cz);
    const B: usize = llamacraft::chunk::CHUNK_SX * llamacraft::chunk::CHUNK_SZ;
    let mut out = Vec::with_capacity(8 + VOLUME + B);
    out.extend_from_slice(&cx.to_le_bytes());
    out.extend_from_slice(&cz.to_le_bytes());
    out.extend_from_slice(chunk.blocks_slice());
    out.extend_from_slice(chunk.biomes_slice());
    out
}

fn generate_cached(seed: u32, cx: i32, cz: i32) -> llamacraft::chunk::Chunk {
    GENERATOR.with(|cell| {
        let mut cached = cell.borrow_mut();
        match cached.as_mut() {
            Some((cached_seed, generator)) if *cached_seed == seed => {
                generate_chunk_with(generator, cx, cz)
            }
            _ => {
                *cached = Some((seed, ChunkGenerator::new(seed)));
                let (_, generator) = cached.as_mut().expect("generator was just installed");
                generate_chunk_with(generator, cx, cz)
            }
        }
    })
}

#[wasm_bindgen(start)]
pub fn _start() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Warn).ok();
}
