//! Web Worker entry: receives `GenRequest {cx,cz,seed}` (12 bytes LE)
//! via `postMessage`, runs `generate_chunk`, replies with 8 + VOLUME bytes.

use llamacraft::chunk::VOLUME;
use llamacraft::worldgen::generate_chunk;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn worker_entry(payload: &[u8]) -> Vec<u8> {
    console_error_panic_hook::set_once();
    if payload.len() < 12 { return Vec::new(); }
    let cx = i32::from_le_bytes(payload[0..4].try_into().unwrap());
    let cz = i32::from_le_bytes(payload[4..8].try_into().unwrap());
    let seed = u32::from_le_bytes(payload[8..12].try_into().unwrap());
    let chunk = generate_chunk(seed, cx, cz);
    const B: usize = llamacraft::chunk::CHUNK_SX * llamacraft::chunk::CHUNK_SZ;
    let mut out = Vec::with_capacity(8 + VOLUME + B);
    out.extend_from_slice(&cx.to_le_bytes());
    out.extend_from_slice(&cz.to_le_bytes());
    out.extend_from_slice(chunk.blocks_slice());
    out.extend_from_slice(chunk.biomes_slice());
    out
}

#[wasm_bindgen(start)]
pub fn _start() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Warn).ok();
}