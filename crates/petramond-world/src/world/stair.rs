use crate::block_state::StairState;
use crate::mathh::IVec3;
use crate::stair::StairShape;

use super::data::WorldData;

impl WorldData {
    #[inline]
    pub fn stair_state_at(&self, wx: i32, wy: i32, wz: i32) -> StairState {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.stair_state(lx, ly, lz),
            None => StairState::default(),
        }
    }

    #[inline]
    pub fn stair_shape_at(&self, wx: i32, wy: i32, wz: i32) -> StairShape {
        use crate::block::CellView;
        StairShape::from_cell(crate::block::ShapeNeighborhood::shape_state(
            self,
            IVec3::new(wx, wy, wz),
        ))
    }
}
