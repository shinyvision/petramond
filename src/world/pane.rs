//! Glass panes at the world level: position-aware connection masks and boxes.
//!
//! A pane keeps no per-cell state — its connections are resolved from the
//! current neighbours on every query (like stair corners), so collision,
//! selection, and the placement overlap check all read the same
//! `crate::pane::resolved_mask` the mesher renders from.

use crate::block::{Aabb, ConnectionRule, ShapeFamily};
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The 4-bit connection mask of the pane placed at `pos`, using its own rule
    /// (a modded bar may connect differently than an engine pane). See
    /// [`World::connection_mask_at`](crate::world::World::connection_mask_at).
    #[inline]
    pub fn pane_mask_at(&self, pos: IVec3) -> u8 {
        let block = self.physics_block(pos.x, pos.y, pos.z);
        debug_assert_eq!(
            block.shape_family(),
            ShapeFamily::Pane,
            "pane_mask_at on a non-pane cell"
        );
        // Silent default in release (see the fence twin): a non-pane cell falls
        // back to the engine rule, the debug assert catches the misuse.
        let rule = block
            .shape_kind_def()
            .params
            .connection()
            .map_or(ConnectionRule::SolidOrSame, |c| c.rule);
        self.connection_mask_at(pos, ShapeFamily::Pane, rule)
    }

    /// The collision/selection boxes for the pane placed at `pos`, from its own
    /// connection params.
    #[inline]
    pub fn pane_boxes_at(&self, pos: IVec3) -> &'static [Aabb] {
        let block = self.physics_block(pos.x, pos.y, pos.z);
        debug_assert_eq!(
            block.shape_family(),
            ShapeFamily::Pane,
            "pane_boxes_at on a non-pane cell"
        );
        match block.shape_kind_def().params.connection() {
            Some(c) => self.connection_boxes_at(pos, c, ShapeFamily::Pane),
            None => &[],
        }
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
    fn pane_connects_to_full_cubes_and_panes_but_not_tagged_irregulars() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        assert_eq!(w.pane_mask_at(p), 0, "isolated pane is a bare post");

        w.set_block_world(7, 64, 8, Block::Stone);
        w.set_block_world(9, 64, 8, Block::GlassPane);
        assert_eq!(w.pane_mask_at(p), crate::pane::WEST | crate::pane::EAST);

        w.set_block_world(8, 64, 7, Block::Chest);
        w.set_block_world(8, 64, 9, Block::Cactus);
        assert_eq!(
            w.pane_mask_at(p),
            crate::pane::WEST | crate::pane::EAST,
            "no_pane_connect blocks must not add arms"
        );
    }

    #[test]
    fn pane_connects_to_a_stair_back_but_not_its_open_side() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // Stair east of the pane, facing east: its flat high/back side faces the pane.
        assert!(w.place_stair(
            IVec3::new(9, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.pane_mask_at(p), crate::pane::EAST);

        // Stair west of the pane, also facing east: its open side faces the pane.
        assert!(w.place_stair(
            IVec3::new(7, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.pane_mask_at(p), crate::pane::EAST);
    }

    #[test]
    fn pane_connects_to_a_full_slab_stack_but_not_a_single_slab() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        let n = IVec3::new(8, 64, 7);
        let slot = |index| crate::slab::SlabSlot {
            split: SlabSplit::Y,
            index,
        };
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(0)));
        assert_eq!(w.pane_mask_at(p), 0, "a single slab is not a full face");
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(1)));
        assert_eq!(w.pane_mask_at(p), crate::pane::NORTH);
    }
}
