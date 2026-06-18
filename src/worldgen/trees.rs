//! Five oak variants.
//!
//! Strata P0: relocated verbatim from `gen.rs` `mod trees`. P3 dissolves these
//! bespoke functions into a composable `TreeFeature` (trunk placer + foliage
//! placer + config); the RNG draw order here is preserved exactly so that
//! refactor stays byte-parity under the unchanged xorshift64 stream.
//!
//! oak_1: classic 4-5 tall straight, leaf blob.
//! oak_2: taller 6-7 with slight lean.
//! oak_3: 4-tall with canopy wider/offset.
//! oak_4: swamp-style: 5 tall, leaves drooping one block lower on sides.
//! oak_big: procedurally generated: 2x2 trunk, diagonal log branches,
//! layered leaves around canopy corners.

use super::rng::FeatureRng;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SY};

#[derive(Copy, Clone, Debug)]
pub enum OakVariant { Oak1, Oak2, Oak3, Oak4, OakBig }

pub fn place(
    chunk: &mut Chunk, x: usize, y: i32, z: usize,
    v: OakVariant, rng: &mut FeatureRng,
) {
    match v {
        OakVariant::Oak1 => oak_simple(chunk, x, y, z, 4 + rng.next_i32(0,1), 0, 0, rng),
        OakVariant::Oak2 => oak_simple(chunk, x, y, z, 6 + rng.next_i32(0,1), rng.next_i32(-1,1), rng.next_i32(-1,1), rng),
        OakVariant::Oak3 => oak_canopy_offset(chunk, x, y, z, 4, rng),
        OakVariant::Oak4 => oak_swamp(chunk, x, y, z, 5 + rng.next_i32(0,1), rng),
        OakVariant::OakBig => oak_big(chunk, x, y, z, rng),
    }
}

fn log_at(chunk: &mut Chunk, x: i32, y: i32, z: i32) {
    if in_bounds(x, y, z) {
        chunk.set_block_raw(x as usize, y as usize, z as usize, Block::OakLog.id());
    }
}
fn leaf_at(chunk: &mut Chunk, x: i32, y: i32, z: i32) {
    if in_bounds(x, y, z) {
        // Only overwrite air/water.
        let b = chunk.block_raw(x as usize, y as usize, z as usize);
        if b == Block::Air.id() || b == Block::Water.id() {
            chunk.set_block_raw(x as usize, y as usize, z as usize, Block::OakLeaves.id());
        }
    }
}
fn in_bounds(x: i32, y: i32, z: i32) -> bool {
    x >= 0 && x < 16 && z >= 0 && z < 16 && y >= 0 && y < CHUNK_SY as i32
}

/// Classic straight oak with small lean offset.
fn oak_simple(
    chunk: &mut Chunk, x: usize, y: i32, z: usize,
    height: i32, dx: i32, dz: i32, _rng: &mut FeatureRng,
) {
    let mut cx = x as i32;
    let mut cz = z as i32;
    for i in 0..height {
        log_at(chunk, cx, y + i, cz);
        // Apply lean by shifting mid-way up.
        if i == height / 2 { cx += dx; cz += dz; }
    }
    // Leaf blob centered around last 2 logs.
    let top = y + height - 1;
    leaf_blob(chunk, cx, top, cz, 2, false);
}

/// Short oak with a single offset canopy corner (oak_3).
fn oak_canopy_offset(
    chunk: &mut Chunk, x: usize, y: i32, z: usize,
    height: i32, rng: &mut FeatureRng,
) {
    let dx = rng.next_i32(-1, 1);
    let dz = rng.next_i32(-1, 1);
    for i in 0..height {
        log_at(chunk, x as i32, y + i, z as i32);
    }
    let top = y + height - 1;
    // Wider asymmetric blob.
    for ly in -1i32..=2 {
        let r: i32 = if ly <= 0 { 2 } else { 1 };
        for lx in -r..=r {
            for lz in -r..=r {
                if lx == 0 && lz == 0 && ly < 2 { continue; }
                if (lx.abs() == r && lz.abs() == r) && rng.chance(0.5) { continue; }
                leaf_at(chunk, x as i32 + lx + dx * (ly / 2), top + ly, z as i32 + lz + dz * (ly / 2));
            }
        }
    }
}

/// Swamp oak: droopy leaves (one block lower on sides).
fn oak_swamp(
    chunk: &mut Chunk, x: usize, y: i32, z: usize,
    height: i32, rng: &mut FeatureRng,
) {
    for i in 0..height {
        log_at(chunk, x as i32, y + i, z as i32);
    }
    let top = y + height - 1;
    // Top small cap.
    for lx in -1i32..=1 {
        for lz in -1i32..=1 {
            if lx == 0 && lz == 0 { continue; }
            if rng.chance(0.3) { continue; }
            leaf_at(chunk, x as i32 + lx, top + 1, z as i32 + lz);
        }
    }
    // Droopy lower layer.
    for lx in -2i32..=2 {
        for lz in -2i32..=2 {
            if lx.abs() == 2 && lz.abs() == 2 { continue; }
            if rng.chance(0.6) { continue; }
            leaf_at(chunk, x as i32 + lx, top - 1, z as i32 + lz);
        }
    }
    leaf_at(chunk, x as i32, top + 1, z as i32);
}

/// Big oak: 2x2 trunk, procedural log branches, layered leaves.
fn oak_big(chunk: &mut Chunk, x: usize, y: i32, z: usize, rng: &mut FeatureRng) {
    // Reserve 2x2 footprint. Caller already skipped edges.
    if x + 1 >= 16 || z + 1 >= 16 { return; }
    let height = 8 + rng.next_i32(0, 4); // 8..12
    // Trunk: 2x2 column. Logs up to height-2, then single center for crown.
    let base = y;
    for i in 0..height {
        let h = base + i;
        log_at(chunk, x as i32,     h, z as i32);
        log_at(chunk, x as i32 + 1, h, z as i32);
        log_at(chunk, x as i32,     h, z as i32 + 1);
        log_at(chunk, x as i32 + 1, h, z as i32 + 1);
    }
    // Branches: starting at ~70% height, walk 2-3 logs diagonally out/up.
    let crown_base = base + (height * 7 / 10);
    let branch_count = rng.next_i32(2, 4);
    for _ in 0..branch_count {
        let sx = x as i32 + rng.next_i32(0, 1);
        let sz = z as i32 + rng.next_i32(0, 1);
        let sy = crown_base + rng.next_i32(0, 2);
        let (bdx, bdz) = match rng.next_i32(0, 7) {
            0 => (-1,  0), 1 => ( 1,  0), 2 => ( 0, -1), 3 => ( 0,  1),
            4 => (-1, -1), 5 => (-1,  1), 6 => ( 1, -1), _ => ( 1,  1),
        };
        let len = rng.next_i32(2, 4);
        let (mut bx, mut by, mut bz) = (sx, sy, sz);
        for _ in 0..len {
            bx += bdx; by += 1; bz += bdz;
            if in_bounds(bx, by, bz) {
                // Replace leaves if needed.
                let cur = chunk.block_raw(bx as usize, by as usize, bz as usize);
                if cur == Block::Air.id() || cur == Block::OakLeaves.id() || cur == Block::Water.id() {
                    chunk.set_block_raw(bx as usize, by as usize, bz as usize, Block::OakLog.id());
                }
            }
        }
        // Leaf cluster at branch tip.
        leaf_blob(chunk, bx, by, bz, 2, false);
    }
    // Crown: layered leaves around top of trunk (2x2 center).
    let top = base + height - 1;
    let cx = x as i32 + 1;
    let cz = z as i32 + 1;
    // Layer 0 (just below top): radius 2.
    for lx in -2i32..=2 {
        for lz in -2i32..=2 {
            if lx.abs() == 2 && lz.abs() == 2 { continue; }
            leaf_at(chunk, cx + lx, top - 1, cz + lz);
        }
    }
    // Layer 1 (top): radius 1, plus corners randomly.
    for lx in -1i32..=1 {
        for lz in -1i32..=1 {
            if lx == 0 && lz == 0 { leaf_at(chunk, cx, top + 1, cz); continue; }
            if (lx.abs() == 1 && lz.abs() == 1) && rng.chance(0.5) { continue; }
            leaf_at(chunk, cx + lx, top, cz + lz);
        }
    }
    // Layer 2 (above): small cap.
    for lx in -1i32..=1 {
        for lz in -1i32..=1 {
            if lx.abs() == 1 && lz.abs() == 1 { continue; }
            if rng.chance(0.4) { continue; }
            leaf_at(chunk, cx + lx, top + 1, cz + lz);
        }
    }
}

/// Spherical-ish leaf blob centered at (x,y,z).
fn leaf_blob(
    chunk: &mut Chunk, cx: i32, cy: i32, cz: i32,
    radius: i32, allow_overwrite: bool,
) {
    let r = radius;
    for ly in -r..=r {
        for lx in -r..=r {
            for lz in -r..=r {
                let d2 = lx*lx + ly*ly + lz*lz;
                if d2 > r*r + 1 { continue; }
                if d2 > r*r - 1 && (lx.abs() == r || lz.abs() == r || ly.abs() == r) {
                    continue;
                }
                if !in_bounds(cx + lx, cy + ly, cz + lz) { continue; }
                let bx = (cx + lx) as usize;
                let by = (cy + ly) as usize;
                let bz = (cz + lz) as usize;
                if allow_overwrite {
                    chunk.set_block_raw(bx, by, bz, Block::OakLeaves.id());
                } else {
                    let cur = chunk.block_raw(bx, by, bz);
                    if cur == Block::Air.id() || cur == Block::Water.id() {
                        chunk.set_block_raw(bx, by, bz, Block::OakLeaves.id());
                    }
                }
            }
        }
    }
}
