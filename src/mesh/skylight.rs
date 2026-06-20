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
// Each chunk's skylight is computed from ITS OWN blocks (no neighbour reads),
// stored on the Chunk, and recomputed only when that chunk changes (see
// world.rs). Light is on an x2 integer scale (`SKY_FULL` = 30 = level 15): open
// sky = 15. Two terms:
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
// Being self-contained, horizontal light does NOT bleed across chunk borders --
// the dominant vertical sky term stays seamless (per-column); only secondary
// bleed into enclosed spaces can step at a border, and per-vertex border faces
// blend both sides to soften it.

/// How far below the lowest surface to keep solving, so overhang/cave-mouth spill
/// light is captured. Anything deeper just floors to the dark minimum.
const LIGHT_MARGIN_DOWN: i32 = 24;

// Medium codes for the flood buffer.
const M_AIR: u8 = 0; // descent keeps the running rate; horizontal step costs 1 level
const M_LEAF: u8 = 1; // canopy: sets descent rate >= 0.5/block; horizontal step costs 0.5
const M_WATER: u8 = 2; // water: sets descent rate to 1/block; horizontal step costs 1
const M_OPAQUE: u8 = 3; // full cube: blocks light, breaks sky shafts

/// Compute the skylight band for `chunk` from its own blocks. Returns the flat
/// band buffer (x2 light, indexed like blocks with Y offset by `ylo`) plus the
/// band `[ylo, yhi]`. Pure integer flood-fill, order-independent -> deterministic.
/// Reuses per-thread scratch. Call when the chunk's blocks change; the result is
/// stored via `Chunk::set_skylight` and reused across mesh rebuilds.
pub fn compute_chunk_skylight(chunk: &Chunk) -> (Box<[u8]>, i32, i32) {
    const SX: i32 = CHUNK_SX as i32;
    const SZ: i32 = CHUNK_SZ as i32;

    // Vertical band from this chunk's own heightmap.
    let mut hmax = 0i32;
    let mut hmin = CHUNK_SY as i32 - 1;
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let h = chunk.surface_y(x, z);
            if h > hmax {
                hmax = h;
            }
            if h < hmin {
                hmin = h;
            }
        }
    }
    let yhi = (hmax + 1).min(CHUNK_SY as i32 - 1);
    let ylo = (hmin - LIGHT_MARGIN_DOWN).max(0);
    let bh = (yhi - ylo + 1).max(1);
    let vol = (SX * SZ * bh) as usize;

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

    let idx = |x: i32, ay: i32, z: i32| -> usize { ((ay * SZ + z) * SX + x) as usize };

    // Pass 1: fill medium + seed the VOLUMETRIC sky descent. Descend each column
    // from the band top carrying a running `rate` (attenuation per block of
    // descent, x2 scale): rate 0 in open air above any cover, then it ratchets up
    // to 1 (0.5/block) under a canopy and 2 (1/block) under water and KEEPS
    // draining through the air below -- so it gets darker the deeper you go under
    // cover. The first opaque block ends the column (no sky below it; `medium`
    // keeps filling so pass 2 can re-enter caves from the side).
    for z in 0..SZ {
        for x in 0..SX {
            let mut blocked = false;
            let mut cur = SKY_FULL;
            let mut rate = 0u8; // per-block descent attenuation, x2 (0 open / 1 leaf / 2 water)
            let mut wy = yhi;
            while wy >= ylo {
                let b = Block::from_id(chunk.block_raw(x as usize, wy as usize, z as usize));
                let m = if b.is_opaque() {
                    M_OPAQUE
                } else if b == Block::Water {
                    M_WATER
                } else if b == Block::OakLeaves {
                    M_LEAF
                } else {
                    M_AIR
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

    // Pass 2: bucketed Dijkstra (bright -> dark) within the 16x16xbh box. Normal
    // covered neighbours cost 2, leaf-covered neighbours cost 1, and opaque cells
    // are impassable. Frozen pass-1 cells still SOURCE light into enclosed
    // neighbours but are never raised. Staleness check skips voxels already
    // improved past their bucket. Final values are order-independent.
    let mut level = SKY_FULL as i32;
    while level >= 1 {
        while let Some(i) = buckets[level as usize].pop() {
            let iu = i as usize;
            if light2[iu] != level as u8 {
                continue;
            }
            let x = (i % SX as u32) as i32;
            let rem = i / SX as u32;
            let z = (rem % SZ as u32) as i32;
            let ay = (rem / SZ as u32) as i32;
            for (dx, dy, dz) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                let nx = x + dx;
                let ny = ay + dy;
                let nz = z + dz;
                if nx < 0 || nx >= SX || ny < 0 || ny >= bh || nz < 0 || nz >= SZ {
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

    (light2.into_boxed_slice(), ylo, yhi)
}
