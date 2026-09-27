use crate::mathh::IVec3;

use super::data::WorldData;

impl WorldData {
    #[inline]
    pub fn pane_mask_at(&self, pos: IVec3) -> u8 {
        debug_assert!(
            self.physics_block(pos.x, pos.y, pos.z)
                .shape_kind_def()
                .params
                .connection()
                .is_some(),
            "pane_mask_at on a non-connection cell"
        );
        use crate::block::CellView;
        crate::connect::ConnectionMask::from_cell(crate::block::ShapeNeighborhood::shape_state(
            self, pos,
        ))
        .0
    }
}

#[cfg(test)]
mod tests;
