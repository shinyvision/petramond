//! The sim world's implementation of the primitive shape seam.
//!
//! `World` (the server's authoritative world AND the client replica — they are
//! one type) is the sim-side [`ShapeNeighborhood`]: every `ShapeSim` /
//! `ShapeRender` facet resolves through it here, and through the mesher's
//! padded snapshot on the worker thread, with ONE family implementation
//! serving both. Both adapters are a single unified-store read — the seam
//! ships opaque bytes and NEVER interprets them.

use crate::block::{Aabb, Block, ShapeNeighborhood, ShapeRenderBox, ShapeState};
use crate::chunk::section_idx;
use crate::mathh::IVec3;

use super::store::World;

impl ShapeNeighborhood for World {
    fn block(&self, pos: IVec3) -> Block {
        // The sim's authoritative read: an unloaded cell answers the section
        // summary's virtual block (never a panic), exactly what the per-family
        // world accessors read before the seam unified them.
        self.physics_block(pos.x, pos.y, pos.z)
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        // ONE read of the unified store — the seam ships the bytes verbatim;
        // only the family/behavior owning the cell's block decodes them.
        match self.chunk_at_world(pos.x, pos.y, pos.z) {
            Some((c, lx, ly, lz)) => c.cell_state(lx, ly, lz),
            None => ShapeState::NONE,
        }
    }

    fn baked(&self, pos: IVec3) -> Option<&[ShapeRenderBox]> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.shape_render_boxes(section_idx(lx, ly, lz) as u16)
    }

    fn baked_collision(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.custom_shape_boxes(pos)
    }
}
