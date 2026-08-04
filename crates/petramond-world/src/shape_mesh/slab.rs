//! Slab meshing geometry helpers.
//!
//! The chunk mesher draws a slab cell as a plain box set — one half-cell box
//! per material-bearing layer slot ([`slot_box`]) — through the unified
//! `super::boxset` emitter, which owns hidden-face removal (between the
//! cell's own layers, against neighbour slabs/stairs/boxes, and against full
//! occluders) and the per-plane cube lighting.
//!
//! [`slot_box`] is all that remains here: the per-face quad decomposition
//! (`layer_quads`) died with the world-less consumers that wanted it — the
//! break crack and the item icon now resolve through the shape facets
//! (`ShapeRender::boxes` / `item_boxes`) like every other consumer.

use crate::slab::SlabSlot;

/// The cell-local half-cell box one slab layer slot occupies.
pub fn slot_box(slot: SlabSlot) -> ([f32; 3], [f32; 3]) {
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
