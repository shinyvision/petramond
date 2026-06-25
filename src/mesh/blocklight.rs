//! Block-light flood-fill: the light a torch (or any [`light_emission`] block)
//! spreads, cached per chunk in its OWN band — separate from skylight, sized to
//! the emitters, and empty when none are near (so torch-free chunks pay nothing).
//!
//! A bright→dark bucketed Dijkstra seeded at the emitters, attenuating one level
//! (two on the x2 scale) per block through any non-opaque cell and stopped by
//! opaque cubes. Order-independent → deterministic. The light worker runs this
//! alongside the skylight bake; the mesher samples the result next to skylight to
//! brighten and warm-tint torch-lit surfaces.
//!
//! Emitter POSITIONS are supplied by the caller (cheaply gathered from the chunks'
//! torch maps — see `world::light_queue`) rather than discovered by scanning every
//! block, so the common torch-free bake stays free; the EMISSION at each is read
//! from the block id (so a stale position that's no longer a torch contributes 0).
//!
//! [`light_emission`]: crate::block::Block::light_emission

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};
use crate::mathh::IVec3;

/// One block-light step on the x2 scale (= 1 level per block), uniform — block
/// light isn't given skylight's leaf half-rate.
const STEP: i32 = 2;
/// Vertical padding above/below the emitters for the band: how far block light can
/// reach (`SKY_FULL / STEP` = 15 blocks ≥ a level-14 torch's 14).
const REACH: i32 = SKY_FULL as i32 / STEP;
/// One loaded chunk of horizontal halo, so an emitter just across a chunk border
/// still lights this chunk's edge (16 ≥ REACH).
const HALO: i32 = 1;

/// Flood block-light for `chunk` from `emitters` (world block positions), letting
/// it spread through the loaded one-chunk halo. Returns the center chunk's band
/// (x2 light, indexed like blocks with Y offset by `ylo`) and `[ylo, yhi]`. An
/// empty band (with `0, 0`) means no emitter is near.
pub fn compute_chunk_blocklight_with_neighbors<'a>(
    chunk: &'a Chunk,
    neighbour_chunk: impl Fn(i32, i32) -> Option<&'a Chunk>,
    emitters: &[IVec3],
) -> (Box<[u8]>, i32, i32) {
    let grid = HALO * 2 + 1;
    let sx = CHUNK_SX as i32 * grid;
    let sz = CHUNK_SZ as i32 * grid;
    // World coords of buffer cell (0, *, 0).
    let ox = chunk.cx * CHUNK_SX as i32 - HALO * CHUNK_SX as i32;
    let oz = chunk.cz * CHUNK_SZ as i32 - HALO * CHUNK_SZ as i32;
    let in_box = |p: &IVec3| p.x >= ox && p.x < ox + sx && p.z >= oz && p.z < oz + sz;

    // The emitters that fall in the haloed column set the vertical band. None → no
    // block light reaches this chunk; hand back an empty band.
    let mut min_e = i32::MAX;
    let mut max_e = i32::MIN;
    for p in emitters.iter().filter(|p| in_box(p)) {
        min_e = min_e.min(p.y);
        max_e = max_e.max(p.y);
    }
    if min_e == i32::MAX {
        return (Vec::new().into_boxed_slice(), 0, 0);
    }
    let ylo = (min_e - REACH).max(0);
    let yhi = (max_e + REACH).min(CHUNK_SY as i32 - 1);
    let bh = (yhi - ylo + 1).max(1);
    let vol = (sx * sz * bh) as usize;

    let chunk_grid: Vec<Option<&Chunk>> = {
        let mut v = Vec::with_capacity((grid * grid) as usize);
        for dcz in -HALO..=HALO {
            for dcx in -HALO..=HALO {
                v.push(if dcx == 0 && dcz == 0 {
                    Some(chunk)
                } else {
                    neighbour_chunk(chunk.cx + dcx, chunk.cz + dcz)
                });
            }
        }
        v
    };
    // Block at a world voxel, or `None` outside the loaded halo / world bounds.
    let block_at = |wx: i32, wy: i32, wz: i32| -> Option<Block> {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return None;
        }
        let dcx = (wx >> 4) - chunk.cx;
        let dcz = (wz >> 4) - chunk.cz;
        if !(-HALO..=HALO).contains(&dcx) || !(-HALO..=HALO).contains(&dcz) {
            return None;
        }
        let c = chunk_grid[((dcz + HALO) * grid + (dcx + HALO)) as usize]?;
        Some(Block::from_id(c.block_raw(
            (wx & 0x0F) as usize,
            wy as usize,
            (wz & 0x0F) as usize,
        )))
    };
    // Unloaded / out-of-bounds cells count as opaque so light never leaks past the
    // solved region (mirrors the skylight bake's closed boundaries).
    let opaque_at =
        |wx: i32, wy: i32, wz: i32| -> bool { block_at(wx, wy, wz).is_none_or(|b| b.is_opaque()) };

    let idx = |x: i32, ay: i32, z: i32| -> usize { ((ay * sz + z) * sx + x) as usize };
    let mut light2 = vec![0u8; vol];
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); SKY_FULL as usize + 1];

    // Seed every emitter that lies in the solved box at its block's emission.
    for p in emitters.iter().filter(|p| in_box(p)) {
        if p.y < ylo || p.y > yhi {
            continue;
        }
        let e = block_at(p.x, p.y, p.z).map_or(0, Block::light_emission);
        if e == 0 {
            continue;
        }
        let i = idx(p.x - ox, p.y - ylo, p.z - oz);
        if e > light2[i] {
            light2[i] = e;
            buckets[e as usize].push(i as u32);
        }
    }

    // Bright → dark bucketed Dijkstra. Opaque neighbours are impassable; staleness
    // check skips cells already improved past their bucket.
    let mut level = SKY_FULL as i32;
    while level >= 1 {
        while let Some(i) = buckets[level as usize].pop() {
            if light2[i as usize] != level as u8 {
                continue;
            }
            let x = (i % sx as u32) as i32;
            let rem = i / sx as u32;
            let z = (rem % sz as u32) as i32;
            let ay = (rem / sz as u32) as i32;
            for (dx, dy, dz) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                let (nx, ny, nz) = (x + dx, ay + dy, z + dz);
                if !(0..sx).contains(&nx) || ny < 0 || ny >= bh || !(0..sz).contains(&nz) {
                    continue;
                }
                if level <= STEP || opaque_at(ox + nx, ylo + ny, oz + nz) {
                    continue;
                }
                let nl = (level - STEP) as u8;
                let ni = idx(nx, ny, nz);
                if nl > light2[ni] {
                    light2[ni] = nl;
                    buckets[nl as usize].push(ni as u32);
                }
            }
        }
        level -= 1;
    }

    // Copy out the center chunk's 16×16×bh band.
    let cx0 = HALO * CHUNK_SX as i32;
    let cz0 = HALO * CHUNK_SZ as i32;
    let mut out = vec![0u8; (CHUNK_SX * CHUNK_SZ) * bh as usize];
    for ay in 0..bh {
        for z in 0..CHUNK_SZ as i32 {
            for x in 0..CHUNK_SX as i32 {
                let oi = ((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize;
                out[oi] = light2[idx(cx0 + x, ay, cz0 + z)];
            }
        }
    }
    (out.into_boxed_slice(), ylo, yhi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torch_chunk(tx: usize, ty: usize, tz: usize) -> Chunk {
        let mut c = Chunk::new(0, 0);
        c.set_block(tx, ty, tz, Block::Torch);
        c
    }

    #[test]
    fn no_emitters_is_an_empty_band() {
        let c = Chunk::new(0, 0);
        let (band, ylo, yhi) = compute_chunk_blocklight_with_neighbors(&c, |_, _| None, &[]);
        assert!(band.is_empty());
        assert_eq!((ylo, yhi), (0, 0));
    }

    #[test]
    fn a_torch_lights_its_cell_and_falls_off_one_level_per_block() {
        let c = torch_chunk(8, 70, 8);
        let emitters = [IVec3::new(8, 70, 8)];
        let (band, ylo, yhi) = compute_chunk_blocklight_with_neighbors(&c, |_, _| None, &emitters);
        let at = |x: i32, y: i32, z: i32| -> u8 {
            let ay = y - ylo;
            band[((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize]
        };
        assert!(yhi >= 70 && ylo <= 70);
        // The torch cell holds its full emission (level 14 = 28 on the x2 scale)...
        assert_eq!(at(8, 70, 8), Block::Torch.light_emission());
        // ...and air one block away is one level (2 on the x2 scale) dimmer.
        assert_eq!(at(9, 70, 8), Block::Torch.light_emission() - STEP as u8);
        assert_eq!(at(8, 71, 8), Block::Torch.light_emission() - STEP as u8);
        // Far beyond the reach it is dark.
        assert_eq!(at(8, 70, 8 + 7).min(at(1, 70, 1)), 0);
    }

    #[test]
    fn an_opaque_wall_blocks_the_spread() {
        // Torch with a stone wall immediately to its +X: the cell past the wall gets
        // no light through it (it could only be lit by going around).
        let mut c = torch_chunk(8, 70, 8);
        c.set_block(9, 70, 8, Block::Stone);
        let emitters = [IVec3::new(8, 70, 8)];
        let (band, ylo, _yhi) = compute_chunk_blocklight_with_neighbors(&c, |_, _| None, &emitters);
        let at = |x: i32, y: i32, z: i32| -> u8 {
            let ay = y - ylo;
            band[((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize]
        };
        // The stone cell itself is never lit (opaque cells stay 0 in the flood).
        assert_eq!(at(9, 70, 8), 0);
    }

    #[test]
    fn a_furnace_emitter_radiates_like_a_torch() {
        // A furnace is opaque, so it sources light into the surrounding air without
        // the (opaque) furnace cell being walked through. The caller only seeds LIT
        // furnaces (see `world::light_queue`); here we seed it directly to lock that
        // a furnace cell radiates its emission, same level as a torch.
        let mut c = Chunk::new(0, 0);
        c.set_block(8, 70, 8, Block::Furnace);
        let emitters = [IVec3::new(8, 70, 8)];
        let (band, ylo, _yhi) = compute_chunk_blocklight_with_neighbors(&c, |_, _| None, &emitters);
        let at = |x: i32, y: i32, z: i32| -> u8 {
            let ay = y - ylo;
            band[((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize]
        };
        assert_eq!(
            Block::Furnace.light_emission(),
            Block::Torch.light_emission()
        );
        // Air directly above the furnace gets the emission minus one block of falloff.
        assert_eq!(at(8, 71, 8), Block::Furnace.light_emission() - STEP as u8);
    }
}
