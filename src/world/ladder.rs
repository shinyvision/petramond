//! Ladder state at the world level: the wall-support rule and the climbable
//! query the player physics samples.
//!
//! The facing needs no world-level accessor at all: which wall a ladder hangs
//! on is block IDENTITY (one row per facing, `Block::panel_facing` — see
//! `crate::ladder`), so persistence, replication, and the break sweep are the
//! ordinary block-id lanes and callers read the facing off the block they
//! already fetched. This module only adds the ladder-specific support rule and
//! the physics probe. Mirrors [`world::torch`](super::torch).

use crate::block::Block;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// Whether a ladder facing `facing` at `pos` has a usable wall behind it:
    /// the support cell's face toward the ladder must be a complete vertical
    /// face (opaque block, stair back, full slab side — the same rule as the
    /// wall torch). Gates placement, the predicted ghost, and the FRAGILE
    /// support re-check, so all three agree by construction.
    pub(crate) fn ladder_supported_at(&self, pos: IVec3, facing: Facing) -> bool {
        let dir = facing.dir();
        self.wall_face_complete(crate::ladder::support_cell(pos, facing), dir)
    }

    /// The climbable cell sample the player physics probes each sub-step: the
    /// facing of a climbable block at the cell, or `None` when the cell holds
    /// none (or its section is unloaded). One section lookup and a dense flag
    /// read gate it — no `def()` table walk until the cell actually climbs;
    /// the facing then comes off the row of the id already fetched, so no
    /// second per-cell map traversal exists at all.
    pub fn climbable_facing_at(&self, x: i32, y: i32, z: i32) -> Option<Facing> {
        let (s, lx, ly, lz) = self.chunk_at_world(x, y, z)?;
        let block = Block::from_id(s.block_raw(lx, ly, lz));
        block.is_climbable().then(|| block.panel_facing())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{Chunk, ChunkPos};

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn a_ladder_is_supported_only_by_a_complete_wall_face() {
        let mut w = world();
        let ladder = IVec3::new(8, 64, 8);
        // An east-facing ladder hangs on the wall to its west.
        let wall = crate::ladder::support_cell(ladder, Facing::East);
        assert!(
            !w.ladder_supported_at(ladder, Facing::East),
            "no wall, no support"
        );
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        assert!(w.ladder_supported_at(ladder, Facing::East));
        // A wall on a different side does not support this facing.
        assert!(!w.ladder_supported_at(ladder, Facing::North));
    }

    #[test]
    fn a_placed_ladder_collides_as_its_facing_resolved_panel() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        w.set_block_world(p.x, p.y, p.z, Block::LadderEast);
        let boxes = w.collision_boxes_at(p.x, p.y, p.z);
        assert_eq!(boxes, crate::ladder::collision_boxes(Facing::East));
        // The panel is thin, standable geometry hugging the wall side — not a
        // full cube and not empty (a body bumps it and can stand on top).
        assert_eq!(boxes.len(), 1);
        let b = &boxes[0];
        assert!(b.max[0] - b.min[0] < 0.5 || b.max[2] - b.min[2] < 0.5);
        assert_eq!((b.min[1], b.max[1]), (0.0, 1.0));
    }

    #[test]
    fn a_committed_wall_panel_is_the_facing_row_and_no_block_entity() {
        use crate::world::placement::{PlacementPlan, PlacementWrite};
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        let wall = crate::ladder::support_cell(p, Facing::East);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        let plan = PlacementPlan {
            anchor: p,
            cells: vec![p],
            write: PlacementWrite::WallPanel(Facing::East),
        };
        // The shared commit resolves the held (base) row to the facing sibling.
        assert!(w.commit_placement(Block::Ladder, &plan, true));
        assert_eq!(
            Block::from_id(w.chunk_block(p.x, p.y, p.z)),
            Block::LadderEast
        );
        // The point of facing-as-identity: a ladder-only section never
        // classifies as a block-entity section (no per-tick furnace fan-out,
        // no per-frame chest/door collection walks it).
        assert!(
            w.block_entity_sections.is_empty(),
            "a ladder must not index its section as a block-entity section"
        );
    }

    #[test]
    fn climbable_query_reads_the_facing_row() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        assert_eq!(w.climbable_facing_at(p.x, p.y, p.z), None);
        w.set_block_world(p.x, p.y, p.z, Block::LadderSouth);
        assert_eq!(w.climbable_facing_at(p.x, p.y, p.z), Some(Facing::South));
        // A non-climbable block never answers.
        w.set_block_world(p.x, p.y, p.z, Block::Stone);
        assert_eq!(w.climbable_facing_at(p.x, p.y, p.z), None);
    }
}
