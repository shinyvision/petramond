//! Directional stairs at the world level: position-aware facing lookup and placement.

use crate::block::Block;
use crate::furnace::Facing;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The placed facing of the stair at world `pos`, or north for old/non-stair cells.
    #[inline]
    pub fn stair_facing_at(&self, wx: i32, wy: i32, wz: i32) -> Facing {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.stair_facing(lx, ly, lz),
            None => Facing::default(),
        }
    }

    /// Place a single-cell stair and record its facing before relighting/remeshing.
    /// Assumes the caller already gated replaceability and entity overlap.
    pub fn place_stair(&mut self, pos: IVec3, block: Block, facing: Facing) -> bool {
        if !crate::stair::is_stair(block) || !self.materialize_section_at(pos) {
            return false;
        }
        let Some((section_pos, _, _, _)) = Self::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, block);
        section.set_stair_facing(lx, ly, lz, facing);
        section.modified = true;
        if self.update_column_height_after_set(pos.x, pos.y, pos.z, true) {
            self.mark_heightmap_light_dirty_around(section_pos.chunk_pos());
        }
        self.refresh_region(&[pos]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::chunk::ChunkPos;

    #[test]
    fn placing_a_stair_raises_the_column_surface_for_skylight() {
        let mut world = World::new(0, 0);
        let p = IVec3::new(8, 8, 8);

        assert!(world.place_stair(p, Block::OakStairs, Facing::East));

        let column = world.columns.get(&ChunkPos::new(0, 0)).unwrap();
        assert_eq!(
            column.surface_y(8, 8),
            8,
            "a placed stair roof must become sky cover for the column heightmap"
        );
    }
}
