//! Glass panes at the world level: the stored-mask front.
//!
//! A pane's connections are REFINED per-cell state (`ConnectionMask`),
//! resolved by the edit cascade and stored in the cell — every read here is
//! a free decode of the same bytes the mesher renders from.

use crate::mathh::IVec3;

use super::data::WorldData;

impl WorldData {
    /// The refined 4-bit connection mask STORED for the pane placed at `pos`
    /// — a cell-state decode, resolved by the edit cascade, never here.
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
