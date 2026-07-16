use std::cell::RefCell;

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};

struct SkyScratch {
    medium: Vec<u8>,
    sky: Vec<u8>,
    step: Vec<u8>,
    buckets: Vec<Vec<u32>>,
}

thread_local! {
    /// Reusable skylight scratch (the medium buffer, sky-reached flags, and the
    /// Dijkstra step costs/queues), kept per worker thread so the per-chunk
    /// flood-fill doesn't churn the allocator across thousands of streaming mesh
    /// builds. Cleared each use; the result buffer (`light2`) is allocated fresh
    /// since it outlives the solve.
    static SKY_SCRATCH: RefCell<SkyScratch> = const {
        RefCell::new(SkyScratch {
            medium: Vec::new(),
            sky: Vec::new(),
            step: Vec::new(),
            buckets: Vec::new(),
        })
    };
}

// --- Skylight (flood-fill, cached per chunk) -------------------
// Each chunk stores only its own 16x16 light band. The standalone helper computes
// that band from the chunk alone; the world path computes it from the chunk plus
// a loaded one-chunk halo so horizontal flood light can cross chunk borders.
// Light is on an x2 integer scale (`SKY_FULL` = 30 = level 15): open sky = 15.
// Two terms:
//
//  * Vertical sky descent (pass 1, per column) is VOLUMETRIC: a running
//    attenuation `rate` ratchets up the moment skylight enters cover and then
//    keeps draining `rate` per block of DESCENT, even through the air beneath --
//    so it gets darker the deeper you go under water/leaves (and so digging a
//    shaft straight down under cover keeps darkening). Open air above any cover
//    has rate 0 (sky shafts stay 15 to the first cover/opaque block); a canopy
//    sets rate 0.5/block, water sets 1/block (water dominates leaves). The first
//    opaque block ends the column (no sky below it).
//  * Horizontal/secondary bleed (pass 2, bucketed Dijkstra) lights enclosed
//    spaces light can bend into -- caves, tunnels, overhang mouths -- at 1 level
//    per normal covered step, half per leaf-covered step. Opaque-covered cells
//    fill from the side; leaf-covered cells do the same, but with the half-rate
//    cost and with their direct vertical half-rate seed as a floor. Water-lit
//    cells stay frozen so a neighbouring shaft cannot flatten their depth
//    gradient.
//
// The neighbor-aware solve keeps the cached output local, but lets secondary
// bleed step through loaded neighbor blocks before the center band is copied out.

/// How far below the lowest surface to keep solving, so overhang/cave-mouth spill
/// light is captured. Anything deeper just floors to the dark minimum.
const LIGHT_MARGIN_DOWN: i32 = 24;

/// Horizontal halo used by the world bake. Normal air/water bleed costs two x2
/// light units per block, so every possible one-level-per-block cross-border
/// source fits inside the immediate loaded neighbor chunks.
const LIGHT_HALO_CHUNKS: i32 = 1;

// Medium codes for the flood buffer.
const M_AIR: u8 = 0; // descent keeps the running rate; horizontal step costs 1 level
const M_LEAF: u8 = 1; // canopy: sets descent rate >= 0.5/block; horizontal step costs 0.5
const M_WATER: u8 = 2; // water: sets descent rate to 1/block; horizontal step costs 1
const M_OPAQUE: u8 = 3; // full cube: blocks light, breaks sky shafts

/// Compute a standalone skylight band for `chunk` from its own blocks. Returns
/// the flat band buffer (x2 light, indexed like blocks with Y offset by `ylo`)
/// plus the band `[ylo, yhi]`. Pure integer flood-fill, order-independent ->
/// deterministic. Reuses per-thread scratch.
///
/// Test-only: live meshing uses [`compute_chunk_skylight_with_neighbors`]; this
/// neighbour-free variant only sets up skylight in unit tests of other code.
#[cfg(test)]
pub fn compute_chunk_skylight(chunk: &Chunk) -> (Box<[u8]>, i32, i32) {
    compute_chunk_skylight_inner(chunk, 0, |_, _| None)
}

/// Compute the skylight band for `chunk`, allowing horizontal flood light to
/// move through currently loaded neighbor chunks. Missing neighbors are treated
/// as closed boundaries, so unloaded terrain cannot inject temporary cave light.
pub fn compute_chunk_skylight_with_neighbors<'a>(
    chunk: &'a Chunk,
    neighbour_chunk: impl Fn(i32, i32) -> Option<&'a Chunk>,
) -> (Box<[u8]>, i32, i32) {
    compute_chunk_skylight_inner(chunk, LIGHT_HALO_CHUNKS, neighbour_chunk)
}

fn compute_chunk_skylight_inner<'a>(
    chunk: &'a Chunk,
    halo_chunks: i32,
    neighbour_chunk: impl Fn(i32, i32) -> Option<&'a Chunk>,
) -> (Box<[u8]>, i32, i32) {
    let grid_chunks = halo_chunks * 2 + 1;
    let sx = CHUNK_SX as i32 * grid_chunks;
    let sz = CHUNK_SZ as i32 * grid_chunks;
    let origin_x = -halo_chunks * CHUNK_SX as i32;
    let origin_z = -halo_chunks * CHUNK_SZ as i32;

    let mut chunk_grid = Vec::with_capacity((grid_chunks * grid_chunks) as usize);
    for dcz in -halo_chunks..=halo_chunks {
        for dcx in -halo_chunks..=halo_chunks {
            let c = if dcx == 0 && dcz == 0 {
                Some(chunk)
            } else {
                neighbour_chunk(chunk.cx + dcx, chunk.cz + dcz)
            };
            chunk_grid.push(c);
        }
    }

    // Vertical band from the loaded solve area. A tall neighbor must raise the
    // solve top, otherwise the halo could incorrectly seed light below that
    // neighbor's unseen roof.
    let mut hmax = 0i32;
    let mut hmin = CHUNK_SY as i32 - 1;
    for c in chunk_grid.iter().flatten() {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let h = c.surface_y(x, z);
                if h > hmax {
                    hmax = h;
                }
                if h < hmin {
                    hmin = h;
                }
            }
        }
    }
    let yhi = (hmax + 1).min(CHUNK_SY as i32 - 1);
    let ylo = (hmin - LIGHT_MARGIN_DOWN).max(0);
    let bh = (yhi - ylo + 1).max(1);
    let vol = (sx * sz * bh) as usize;

    // Temporary buffers from per-thread scratch (medium, sky, and step are fully
    // overwritten by the fill pass; buckets are cleared). The result band is
    // allocated fresh.
    let (mut medium, mut sky, mut step, mut buckets) = SKY_SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        (
            std::mem::take(&mut s.medium),
            std::mem::take(&mut s.sky),
            std::mem::take(&mut s.step),
            std::mem::take(&mut s.buckets),
        )
    });
    medium.clear();
    medium.resize(vol, M_AIR);
    sky.clear();
    sky.resize(vol, 0);
    step.clear();
    step.resize(vol, 2);
    buckets.resize_with(SKY_FULL as usize + 1, Vec::new);
    for b in buckets.iter_mut() {
        b.clear();
    }
    let mut light2 = vec![0u8; vol];
    // Marks pass-1 cells whose direct vertical value is authoritative. Open sky
    // and water-lit cells freeze here; leaf-covered cells remain fillable by pass
    // 2 so a neighbouring skylight shaft bleeds into them like it does under an
    // opaque roof, but at the leaf half-rate.

    let idx = |x: i32, ay: i32, z: i32| -> usize { ((ay * sz + z) * sx + x) as usize };
    let chunk_at = |rx: i32, rz: i32| -> Option<&Chunk> {
        let dcx = rx >> 4;
        let dcz = rz >> 4;
        if dcx < -halo_chunks || dcx > halo_chunks || dcz < -halo_chunks || dcz > halo_chunks {
            return None;
        }
        let gi = ((dcz + halo_chunks) * grid_chunks + (dcx + halo_chunks)) as usize;
        chunk_grid[gi]
    };

    // Pass 1: fill medium + seed the VOLUMETRIC sky descent. Descend each column
    // from the band top carrying a running `rate` (attenuation per block of
    // descent, x2 scale): rate 0 in open air above any cover, then it ratchets up
    // to 1 (0.5/block) under a canopy and 2 (1/block) under water and KEEPS
    // draining through the air below -- so it gets darker the deeper you go under
    // cover. The first opaque block ends the column (no sky below it; `medium`
    // keeps filling so pass 2 can re-enter caves from the side).
    for z in 0..sz {
        for x in 0..sx {
            let rx = origin_x + x;
            let rz = origin_z + z;
            let owner = chunk_at(rx, rz);
            let mut blocked = false;
            let mut cur = SKY_FULL;
            let mut rate = 0u8; // per-block descent attenuation, x2 (0 open / 1 leaf / 2 water)
            let mut wy = yhi;
            while wy >= ylo {
                let m = match owner {
                    Some(c) => {
                        let b = Block::from_id(c.block_raw(
                            (rx & 0x0F) as usize,
                            wy as usize,
                            (rz & 0x0F) as usize,
                        ));
                        if b.is_opaque() {
                            M_OPAQUE
                        } else if b == Block::Water {
                            M_WATER
                        } else if b == Block::OakLeaves {
                            M_LEAF
                        } else {
                            M_AIR
                        }
                    }
                    None => M_OPAQUE,
                };
                let i = idx(x, wy - ylo, z);
                medium[i] = m;
                step[i] = match m {
                    M_OPAQUE => 0,
                    M_LEAF => 1,
                    _ => 2,
                };
                if !blocked {
                    if m == M_OPAQUE {
                        blocked = true;
                    } else {
                        // Cover ratchets the rate up (water dominates leaves);
                        // open air keeps whatever rate is already in effect.
                        rate = rate.max(match m {
                            M_WATER => 2,
                            M_LEAF => 1,
                            _ => 0,
                        });
                        cur = cur.saturating_sub(rate);
                        light2[i] = cur;
                        step[i] = if rate == 1 { 1 } else { 2 };
                        sky[i] = if rate == 1 { 0 } else { 1 };
                        buckets[cur as usize].push(i as u32);
                    }
                }
                wy -= 1;
            }
        }
    }

    // Pass 2: bucketed Dijkstra (bright -> dark) within the solved box. Normal
    // covered neighbours cost 2, leaf-covered neighbours cost 1, and opaque
    // cells are impassable. Frozen pass-1 cells still SOURCE light into enclosed
    // neighbours but are never raised. Staleness check skips voxels already
    // improved past their bucket. Final values are order-independent.
    let mut level = SKY_FULL as i32;
    while level >= 1 {
        while let Some(i) = buckets[level as usize].pop() {
            let iu = i as usize;
            if light2[iu] != level as u8 {
                continue;
            }
            let x = (i % sx as u32) as i32;
            let rem = i / sx as u32;
            let z = (rem % sz as u32) as i32;
            let ay = (rem / sz as u32) as i32;
            for d in crate::mathh::FACE_NEIGHBORS {
                let nx = x + d.x;
                let ny = ay + d.y;
                let nz = z + d.z;
                if !(0..sx).contains(&nx) || ny < 0 || ny >= bh || !(0..sz).contains(&nz) {
                    continue;
                }
                let ni = idx(nx, ny, nz);
                if sky[ni] != 0 {
                    continue;
                }
                let m = medium[ni];
                if m == M_OPAQUE {
                    continue;
                }
                let cost = step[ni] as i32;
                if level > cost {
                    let nl = (level - cost) as u8;
                    if nl > light2[ni] {
                        light2[ni] = nl;
                        buckets[nl as usize].push(ni as u32);
                    }
                }
            }
        }
        level -= 1;
    }

    // Hand the temporary buffers back for the next build on this thread.
    SKY_SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        s.medium = medium;
        s.sky = sky;
        s.step = step;
        s.buckets = buckets;
    });

    let out_vol = (CHUNK_SX as i32 * CHUNK_SZ as i32 * bh) as usize;
    let mut out = vec![0u8; out_vol];
    let center_x0 = -origin_x;
    let center_z0 = -origin_z;
    for ay in 0..bh {
        for z in 0..CHUNK_SZ as i32 {
            for x in 0..CHUNK_SX as i32 {
                let oi = ((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize;
                out[oi] = light2[idx(center_x0 + x, ay, center_z0 + z)];
            }
        }
    }

    (out.into_boxed_slice(), ylo, yhi)
}
