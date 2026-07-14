//! Ladder state at the world level: world-coordinate access to the ladder's
//! per-cell facing, its wall-support rule, and the climbable query the player
//! physics samples.
//!
//! The facing lives in the section's shared entity-facing map (the chest/furnace
//! front map), so persistence, replication, and the break-time sweep all come
//! from the existing facing plumbing — this module only adds the world wrappers
//! and the ladder-specific support rule. Mirrors [`world::torch`](super::torch).

use crate::block::Block;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// Which way the ladder at `pos` faces (its panel front, away from the wall
    /// it hangs on), or the default if the cell has no recorded facing or its
    /// chunk is unloaded.
    pub fn ladder_facing(&self, pos: IVec3) -> Facing {
        match self.chunk_at_world(pos.x, pos.y, pos.z) {
            Some((c, lx, ly, lz)) => c.entity_facing(lx, ly, lz),
            None => Facing::default(),
        }
    }

    /// Whether a ladder facing `facing` at `pos` has a usable wall behind it:
    /// the support cell's face toward the ladder must be a complete vertical
    /// face (opaque block, stair back, full slab side — the same rule as the
    /// wall torch). Gates placement, the predicted ghost, and the FRAGILE
    /// support re-check, so all three agree by construction.
    pub(crate) fn ladder_supported_at(&self, pos: IVec3, facing: Facing) -> bool {
        let dir = crate::ladder::facing_dir(facing);
        self.wall_face_complete(crate::ladder::support_cell(pos, facing), dir)
    }

    /// The climbable cell sample the player physics probes each sub-step: the
    /// facing of a climbable block at the cell, or `None` when the cell holds
    /// none (or its section is unloaded). One section lookup and a dense flag
    /// read, so the probe costs what `water_cell_at` costs — no `def()` table
    /// walk, no second map traversal for the facing.
    pub fn climbable_facing_at(&self, x: i32, y: i32, z: i32) -> Option<Facing> {
        let (s, lx, ly, lz) = self.chunk_at_world(x, y, z)?;
        Block::from_id(s.block_raw(lx, ly, lz))
            .is_climbable()
            .then(|| s.entity_facing(lx, ly, lz))
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
        w.set_block_world(p.x, p.y, p.z, Block::Ladder);
        w.insert_entity_facing(p, Facing::East);
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
    fn climbable_query_reads_the_placed_facing() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        assert_eq!(w.climbable_facing_at(p.x, p.y, p.z), None);
        w.set_block_world(p.x, p.y, p.z, Block::Ladder);
        w.insert_entity_facing(p, Facing::South);
        assert_eq!(w.climbable_facing_at(p.x, p.y, p.z), Some(Facing::South));
        // A non-climbable block never answers, whatever facing the cell holds.
        w.set_block_world(p.x, p.y, p.z, Block::Stone);
        assert_eq!(w.climbable_facing_at(p.x, p.y, p.z), None);
    }
}
