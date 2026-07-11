//! Slab meshing: one or two material-bearing half-cell cuboids in a block cell.

use crate::atlas::Tile;
use crate::block::Block;
use crate::block_state::SlabState;
use crate::slab::SlabSlot;

use super::builder::cube_face_lighting;
use super::face::{Face, FACES};
use super::plane::{push_plane_quad, PlaneLight, PlaneQuads};
use super::vertex::Vertex;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_slab_block<B, S, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    state: SlabState,
    tint_for: &T,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    T: Fn(Tile) -> [f32; 3],
{
    for (slot, block) in crate::slab::layer_slots(state) {
        for face in FACES {
            emit_layer_face(
                opaque,
                opaque_idx,
                wx,
                wy,
                wz,
                state,
                slot,
                block,
                face,
                tint_for,
                block_at,
                slab_at,
                neighbour_light,
                neighbour_blocklight,
            );
        }
    }
}

/// Exterior quads for one slab layer face, culling only against the same cell.
/// Used by item rendering where there are no world neighbours.
pub(crate) fn layer_quads(state: SlabState, slot: SlabSlot, face: Face) -> PlaneQuads {
    layer_quads_with(state, slot, face, |_, _, _| false)
}

#[allow(clippy::too_many_arguments)]
fn emit_layer_face<B, S, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    state: SlabState,
    slot: SlabSlot,
    block: Block,
    face: Face,
    tint_for: &T,
    block_at: &B,
    slab_at: &S,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    S: Fn(i32, i32, i32) -> Option<SlabState>,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    T: Fn(Tile) -> [f32; 3],
{
    let (dx, dy, dz) = face.dir();
    let nb_pos = (wx + dx, wy + dy, wz + dz);
    let nb = block_at(nb_pos.0, nb_pos.1, nb_pos.2);
    let nb_slab = crate::slab::is_slab(nb)
        .then(|| slab_at(nb_pos.0, nb_pos.1, nb_pos.2))
        .flatten();
    let (quads, n) = layer_quads_with(state, slot, face, |ix, iy, iz| {
        nb.is_opaque() || nb_slab.is_some_and(|s| neighbour_occupies_boundary(s, face, ix, iy, iz))
    });
    if n == 0 {
        return;
    }

    let tiles = block.tiles();
    let tile = match face {
        Face::PosY => tiles[0],
        Face::NegY => tiles[1],
        _ => tiles[2],
    };
    let tint = tint_for(tile);

    for &(min, max) in quads.iter().take(n) {
        let (fx, fy, fz) = front_voxel(wx, wy, wz, face, min, max);
        let (ao, sky, block_light, warm) = cube_face_lighting(
            face,
            fx,
            fy,
            fz,
            neighbour_light(fx, fy, fz) as u32,
            neighbour_blocklight(fx, fy, fz) as u32,
            face != Face::NegY,
            block_at,
            slab_at,
            neighbour_light,
            neighbour_blocklight,
        );
        push_plane_quad(
            opaque,
            opaque_idx,
            wx,
            wy,
            wz,
            min,
            max,
            face,
            tile,
            tint,
            &PlaneLight {
                ao,
                sky,
                block: block_light,
                warm,
            },
        );
    }
}

fn layer_quads_with(
    state: SlabState,
    slot: SlabSlot,
    face: Face,
    boundary_occupied: impl Fn(usize, usize, usize) -> bool,
) -> PlaneQuads {
    let mut cells: [([f32; 3], [f32; 3]); 4] = Default::default();
    let mut n = 0;
    let (mut lo, mut hi) = ([usize::MAX; 3], [usize::MIN; 3]);
    for iy in 0..2 {
        for iz in 0..2 {
            for ix in 0..2 {
                if !slot_occupies(slot, ix, iy, iz) {
                    continue;
                }
                let (dx, dy, dz) = face.dir();
                let nx = ix as i32 + dx;
                let ny = iy as i32 + dy;
                let nz = iz as i32 + dz;
                let hidden = if (0..2).contains(&nx) && (0..2).contains(&ny) && (0..2).contains(&nz)
                {
                    crate::slab::half_cell_occupied(state, nx as usize, ny as usize, nz as usize)
                } else {
                    boundary_occupied(ix, iy, iz)
                };
                if hidden {
                    continue;
                }
                cells[n] = crate::slab::half_cell_bounds(ix, iy, iz);
                n += 1;
                for (axis, v) in [ix, iy, iz].into_iter().enumerate() {
                    lo[axis] = lo[axis].min(v);
                    hi[axis] = hi[axis].max(v);
                }
            }
        }
    }
    if n == 0 {
        return (cells, 0);
    }
    let bbox_cells: usize = (0..3).map(|axis| hi[axis] - lo[axis] + 1).product();
    if n == bbox_cells {
        let min = [0, 1, 2].map(|axis| lo[axis] as f32 * 0.5);
        let max = [0, 1, 2].map(|axis| (hi[axis] as f32 + 1.0) * 0.5);
        cells[0] = (min, max);
        n = 1;
    }
    (cells, n)
}

#[inline]
fn slot_occupies(slot: SlabSlot, ix: usize, iy: usize, iz: usize) -> bool {
    match slot.split {
        crate::block_state::SlabSplit::X => ix == slot.index,
        crate::block_state::SlabSplit::Y => iy == slot.index,
        crate::block_state::SlabSplit::Z => iz == slot.index,
    }
}

#[inline]
fn neighbour_occupies_boundary(
    state: SlabState,
    face: Face,
    ix: usize,
    iy: usize,
    iz: usize,
) -> bool {
    let (dx, dy, dz) = face.dir();
    let nx = boundary_coord(ix, dx);
    let ny = boundary_coord(iy, dy);
    let nz = boundary_coord(iz, dz);
    crate::slab::half_cell_occupied(state, nx, ny, nz)
}

#[inline]
fn boundary_coord(c: usize, d: i32) -> usize {
    if d > 0 {
        0
    } else if d < 0 {
        1
    } else {
        c
    }
}

#[inline]
fn front_voxel(
    wx: i32,
    wy: i32,
    wz: i32,
    face: Face,
    min: [f32; 3],
    max: [f32; 3],
) -> (i32, i32, i32) {
    let (dx, dy, dz) = face.dir();
    let outer = match face {
        Face::PosX => max[0] >= 1.0,
        Face::NegX => min[0] <= 0.0,
        Face::PosY => max[1] >= 1.0,
        Face::NegY => min[1] <= 0.0,
        Face::PosZ => max[2] >= 1.0,
        Face::NegZ => min[2] <= 0.0,
    };
    if outer {
        (wx + dx, wy + dy, wz + dz)
    } else {
        (wx, wy, wz)
    }
}
