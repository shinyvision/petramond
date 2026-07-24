//! The sim world's implementation of the primitive shape seam.
//!
//! `World` (the server's authoritative world AND the client replica — they are
//! one type) is the sim-side [`ShapeNeighborhood`]: every `ShapeSim` /
//! `ShapeRender` facet resolves through it here, and through the mesher's
//! padded snapshot on the worker thread, with ONE family implementation
//! serving both. Both adapters are a single unified-store read — the seam
//! ships opaque bytes and NEVER interprets them.

use crate::block::{Aabb, Block, CellPart, ShapeNeighborhood, ShapeRenderBox, ShapeState};
use crate::chunk::section_idx;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The sub-cell parts the cell at `pos` is composed of, each with the
    /// block it is made of — the family's own answer (see `ShapeSim::parts`).
    /// `None` means the cell is one whole part of its own block, which is
    /// every family but a stacking slab.
    pub fn cell_parts(&self, pos: IVec3) -> Option<Vec<(CellPart, Block)>> {
        let block = self.physics_block(pos.x, pos.y, pos.z);
        let k = block.shape_kind_def();
        k.sim.parts(&k.params, self, pos, block)
    }

    /// The tint the cell at `pos` presents for a break burst: the first part
    /// carrying one, in part order. A single-part cell is just its bare
    /// `petramond:tint`; a two-tone slab cell bursts in whichever layer is
    /// dyed rather than in nothing at all.
    pub fn cell_burst_tint(&self, pos: IVec3) -> Option<[u8; 3]> {
        let read = |part: CellPart| {
            self.cell_kv_get(
                pos.x,
                pos.y,
                pos.z,
                &crate::block::part_kv_key(crate::block::TINT_KV_KEY, part),
            )
            .and_then(|v| <[u8; 3]>::try_from(v).ok())
        };
        match self.cell_parts(pos) {
            Some(parts) => parts.into_iter().find_map(|(part, _)| read(part)),
            None => read(0),
        }
    }
}

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
