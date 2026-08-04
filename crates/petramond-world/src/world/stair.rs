//! Directional stairs at the world level: position-aware facing lookup and placement.
//! (Data-half queries; the mutation/orchestration half stays in the engine crate.)

use crate::block_state::StairState;
use crate::mathh::IVec3;
use crate::stair::StairShape;

    
    
    
use super::data::WorldData;

impl WorldData {
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

}
