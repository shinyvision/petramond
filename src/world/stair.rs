//! Directional stairs at the world level: position-aware facing lookup and placement.

use crate::block::{Aabb, Block};
use crate::block_state::StairState;
use crate::mathh::IVec3;
use crate::stair::StairShape;

use super::store::World;

impl World {
    /// The placed facing of the stair at world `pos`, or north for old/non-stair cells.
    #[inline]
    pub fn stair_state_at(&self, wx: i32, wy: i32, wz: i32) -> StairState {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.stair_state(lx, ly, lz),
            None => StairState::default(),
        }
    }

    /// The collision/render boxes for a stair as shaped by adjacent perpendicular
    /// stairs. Corner-ness is derived, not saved, so old worlds remain compatible.
    #[inline]
    pub fn stair_boxes_at(&self, wx: i32, wy: i32, wz: i32) -> &'static [Aabb] {
        let state = self.stair_state_at(wx, wy, wz);
        self.resolved_stair_boxes(IVec3::new(wx, wy, wz), state)
    }

    /// Resolve the boxes a stair with `facing` would have at `pos`, using the current
    /// neighbouring world state. Used both after placement and for placement overlap
    /// checks before the block is written.
    #[inline]
    pub fn resolved_stair_boxes(&self, pos: IVec3, state: StairState) -> &'static [Aabb] {
        crate::stair::resolved_boxes_state(pos, state, |p| self.stair_state_if_stair(p))
    }

    /// The corner-resolved shape of the stair at `pos` — the same shape
    /// the chunk mesher renders from, so mask consumers (the break-crack overlay)
    /// derive exactly the meshed shape.
    #[inline]
    pub fn stair_shape_at(&self, wx: i32, wy: i32, wz: i32) -> StairShape {
        let state = self.stair_state_at(wx, wy, wz);
        crate::stair::resolved_shape(IVec3::new(wx, wy, wz), state, |p| {
            self.stair_state_if_stair(p)
        })
    }

    #[inline]
    fn stair_state_if_stair(&self, pos: IVec3) -> Option<StairState> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        crate::stair::is_stair(block).then(|| self.stair_state_at(pos.x, pos.y, pos.z))
    }

    /// Place a single-cell stair and record its facing before relighting/remeshing.
    /// Assumes the caller already gated replaceability and entity overlap.
    pub fn place_stair(&mut self, pos: IVec3, block: Block, state: StairState) -> bool {
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
        section.set_stair_state(lx, ly, lz, state);
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

    use crate::block_state::StairHalf;
    use crate::chunk::ChunkPos;

    #[test]
    fn placing_a_stair_raises_the_column_surface_for_skylight() {
        let mut world = World::new(0, 0);
        let p = IVec3::new(8, 8, 8);

        assert!(world.place_stair(
            p,
            Block::OakStairs,
            StairState::new(crate::furnace::Facing::East, StairHalf::Bottom)
        ));

        let column = world.columns.get(&ChunkPos::new(0, 0)).unwrap();
        assert_eq!(
            column.surface_y(8, 8),
            8,
            "a placed stair roof must become sky cover for the column heightmap"
        );
    }
}
