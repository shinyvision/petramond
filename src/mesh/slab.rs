//! Slab meshing geometry helpers.
//!
//! The chunk mesher draws a slab cell as a plain box set — one half-cell box
//! per material-bearing layer slot ([`slot_box`]) — through the unified
//! [`super::boxset`] emitter, which owns hidden-face removal (between the
//! cell's own layers, against neighbour slabs/stairs/boxes, and against full
//! occluders) and the per-plane cube lighting.
//!
//! [`layer_quads`] remains the PURE per-face quad decomposition at half-cell
//! granularity for contexts without world neighbours: the item cube and the
//! break-crack overlay.

use crate::block_state::SlabState;
use crate::slab::SlabSlot;

use super::face::Face;
use super::plane::PlaneQuads;

/// The cell-local half-cell box one slab layer slot occupies.
pub(super) fn slot_box(slot: SlabSlot) -> ([f32; 3], [f32; 3]) {
    let mut min = [0.0f32; 3];
    let mut max = [1.0f32; 3];
    let axis = match slot.split {
        crate::block_state::SlabSplit::X => 0,
        crate::block_state::SlabSplit::Y => 1,
        crate::block_state::SlabSplit::Z => 2,
    };
    min[axis] = slot.index as f32 * 0.5;
    max[axis] = min[axis] + 0.5;
    (min, max)
}

/// Exterior quads for one slab layer face, culling only against the same cell.
/// Used by item rendering and the break overlay, where there are no world
/// neighbours.
pub(crate) fn layer_quads(state: SlabState, slot: SlabSlot, face: Face) -> PlaneQuads {
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
                let hidden = (0..2).contains(&nx)
                    && (0..2).contains(&ny)
                    && (0..2).contains(&nz)
                    && crate::slab::half_cell_occupied(state, nx as usize, ny as usize, nz as usize);
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
