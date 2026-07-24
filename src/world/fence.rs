//! Fences at the world level: the stored-mask front.
//!
//! A fence's connections are REFINED per-cell state (`ConnectionMask`),
//! resolved by the edit cascade and stored in the cell — every read here is
//! a free decode of the same bytes the mesher renders from.

use crate::block::ShapeFamily;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The refined 4-bit connection mask STORED for the fence placed at `pos`
    /// — a cell-state decode, resolved by the edit cascade, never here.
    #[inline]
    pub fn fence_mask_at(&self, pos: IVec3) -> u8 {
        debug_assert_eq!(
            self.physics_block(pos.x, pos.y, pos.z).shape_family(),
            ShapeFamily::Fence,
            "fence_mask_at on a non-fence cell"
        );
        use crate::block::CellView;
        crate::connect::ConnectionMask::from_cell(crate::block::ShapeNeighborhood::shape_state(
            self, pos,
        ))
        .0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::block_state::{SlabSplit, StairHalf, StairState};
    use crate::chunk::{Chunk, ChunkPos};
    use crate::facing::Facing;

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn fence_connects_to_opaque_cubes_and_fences_but_not_transparent_blocks() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::OakFence));
        assert_eq!(w.fence_mask_at(p), 0, "isolated fence is a bare post");

        w.set_block_world(7, 64, 8, Block::Stone);
        w.set_block_world(9, 64, 8, Block::OakFence);
        assert_eq!(w.fence_mask_at(p), crate::pane::WEST | crate::pane::EAST);

        w.set_block_world(7, 64, 8, Block::OakLeaves);
        w.set_block_world(8, 64, 9, Block::Glass);
        assert_eq!(
            w.fence_mask_at(p),
            crate::pane::EAST,
            "transparent blocks must not grow fence arms"
        );
    }

    #[test]
    fn fence_connects_to_a_stair_back_but_not_its_open_side() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::OakFence));
        // Stair east of the fence, facing east: its flat high/back side faces the fence.
        assert!(w.place_stair(
            IVec3::new(9, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.fence_mask_at(p), crate::pane::EAST);

        // Stair west of the fence, also facing east: its open side faces the fence.
        assert!(w.place_stair(
            IVec3::new(7, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.fence_mask_at(p), crate::pane::EAST);
    }

    #[test]
    fn fence_connects_to_a_full_slab_stack_but_not_a_single_slab() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::OakFence));
        let n = IVec3::new(8, 64, 7);
        let slot = |index| crate::slab::SlabSlot {
            split: SlabSplit::Y,
            index,
        };
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(0)));
        assert_eq!(w.fence_mask_at(p), 0, "a single slab is not a full face");
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(1)));
        assert_eq!(w.fence_mask_at(p), crate::pane::NORTH);
    }
}
