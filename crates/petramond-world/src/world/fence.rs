//! Fences at the world level: the stored-mask front.
//!
//! A fence's connections are REFINED per-cell state (`ConnectionMask`),
//! resolved by the edit cascade and stored in the cell — every read here is
//! a free decode of the same bytes the mesher renders from.

use crate::block::ShapeFamily;
use crate::mathh::IVec3;

use super::data::WorldData;

impl WorldData {
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

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    #[allow(unused_imports)]
    pub use super::*;
    pub use crate::mathh::IVec3;
}
