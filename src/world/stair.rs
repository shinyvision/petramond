//! Directional stairs at the world level: position-aware facing lookup and placement.

use crate::block::Block;
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

    /// The refined corner shape STORED for the stair at `pos` — the same
    /// bytes the chunk mesher renders from, so mask consumers (the
    /// break-crack overlay) decode exactly the meshed shape. Resolved by the
    /// edit cascade, never here.
    #[inline]
    pub fn stair_shape_at(&self, wx: i32, wy: i32, wz: i32) -> StairShape {
        use crate::block::CellView;
        StairShape::from_cell(crate::block::ShapeNeighborhood::shape_state(
            self,
            IVec3::new(wx, wy, wz),
        ))
    }

    /// Place a single-cell stair and record its facing before relighting/remeshing.
    /// Assumes the caller already gated replaceability and entity overlap.
    pub fn place_stair(&mut self, pos: IVec3, block: Block, state: StairState) -> bool {
        if !crate::stair::is_stair(block) || !self.materialize_section_at(pos) {
            return false;
        }
        let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, block);
        section.set_stair_state(lx, ly, lz, state);
        section.modified = true;
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
            StairState::new(crate::facing::Facing::East, StairHalf::Bottom)
        ));

        let column = world.columns.get(&ChunkPos::new(0, 0)).unwrap();
        assert_eq!(
            column.surface_y(8, 8),
            8,
            "a placed stair roof must become sky cover for the column heightmap"
        );
    }
}
