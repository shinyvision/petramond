//! Fences at the world level: position-aware connection masks and boxes.
//!
//! A fence keeps no per-cell state — its connections are resolved from the
//! current neighbours on every query (like stair corners), so collision,
//! selection, and the placement overlap check all read the same
//! `crate::fence::resolved_mask` the mesher renders from.

use crate::block::Aabb;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The 4-bit connection mask a fence at `pos` has (or WOULD have — the cell's
    /// current content is never read, so placement can ask before writing).
    #[inline]
    pub fn fence_mask_at(&self, pos: IVec3) -> u8 {
        crate::fence::resolved_mask(
            pos,
            |p| self.physics_block(p.x, p.y, p.z),
            |p| self.stair_shape_at(p.x, p.y, p.z),
            |p| self.slab_state_at(p.x, p.y, p.z).is_full(),
        )
    }

    /// The collision/selection boxes for a fence at `pos`, shaped by its current
    /// neighbours: the centre post, extended by full-height arms toward each
    /// connected side.
    #[inline]
    pub fn fence_boxes_at(&self, pos: IVec3) -> &'static [Aabb] {
        crate::fence::boxes_for_mask(self.fence_mask_at(pos))
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
