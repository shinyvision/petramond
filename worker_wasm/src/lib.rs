//! Web Worker entry: receives `GenRequest {cx,cz,seed}` (12 bytes LE)
//! via `postMessage`, runs chunk generation, replies with 8 + VOLUME bytes.

use std::cell::RefCell;
use std::collections::HashMap;

use llamacraft::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ, VOLUME};
use llamacraft::mesh::compute_chunk_skylight_with_neighbors;
use llamacraft::worldgen::{driver::ChunkGenerator, generate_chunk_with};
use wasm_bindgen::prelude::*;

const LIGHT_REQ_TAG: u8 = b'L';
const LIGHT_RES_TAG: u8 = b'l';

thread_local! {
    static GENERATOR: RefCell<Option<(u32, ChunkGenerator)>> = RefCell::new(None);
}

#[wasm_bindgen]
pub fn worker_entry(payload: &[u8]) -> Vec<u8> {
    console_error_panic_hook::set_once();
    if payload.first().copied() == Some(LIGHT_REQ_TAG) {
        return light_worker_entry(payload).unwrap_or_default();
    }
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

fn light_worker_entry(payload: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(payload.get(1..)?);
    let id = r.u64()?;
    let cx = r.i32()?;
    let cz = r.i32()?;
    let revision = r.u64()?;
    let neighbour_count = r.u8()? as usize;
    let chunk = r.chunk()?;
    let mut neighbours = HashMap::with_capacity(neighbour_count);
    for _ in 0..neighbour_count {
        let c = r.chunk()?;
        neighbours.insert(ChunkPos::new(c.cx, c.cz), c);
    }

    let (band, ylo, yhi) = compute_chunk_skylight_with_neighbors(&chunk, |nx, nz| {
        neighbours.get(&ChunkPos::new(nx, nz))
    });
    let mut out = Vec::with_capacity(1 + 8 + 8 + 8 + 8 + 4 + band.len());
    out.push(LIGHT_RES_TAG);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(&cx.to_le_bytes());
    out.extend_from_slice(&cz.to_le_bytes());
    out.extend_from_slice(&revision.to_le_bytes());
    out.extend_from_slice(&ylo.to_le_bytes());
    out.extend_from_slice(&yhi.to_le_bytes());
    out.extend_from_slice(&(band.len() as u32).to_le_bytes());
    out.extend_from_slice(&band);
    Some(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    off: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, off: 0 }
    }

    fn u8(&mut self) -> Option<u8> {
        let b = *self.bytes.get(self.off)?;
        self.off += 1;
        Some(b)
    }

    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.array()?))
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.array()?))
    }

    fn chunk(&mut self) -> Option<Chunk> {
        let cx = self.i32()?;
        let cz = self.i32()?;
        let mut chunk = Chunk::new(cx, cz);
        for i in 0..CHUNK_SX * CHUNK_SZ {
            chunk.heightmap[i] = self.u16()?;
        }
        chunk.blocks_slice_mut().copy_from_slice(self.take(VOLUME)?);
        chunk.dirty = false;
        chunk.light_dirty = false;
        Some(chunk)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.off.checked_add(len)?;
        let out = self.bytes.get(self.off..end)?;
        self.off = end;
        Some(out)
    }
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
