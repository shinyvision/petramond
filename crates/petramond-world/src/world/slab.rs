//! Stackable slabs at the world level: position-aware state lookup, collision,
//! and placement.
//! (Data-half queries; the mutation/orchestration half stays in the engine crate.)

use crate::block::Block;
use crate::block_state::SlabState;
use crate::mathh::IVec3;
use crate::slab::SlabSlot;

use super::data::WorldData;

impl WorldData {
    /// The placed slab state at world `pos`, defaulting old/synthetic slab cells to a
    /// bottom-half slab of their own block material.
    #[inline]
    pub fn slab_state_at(&self, wx: i32, wy: i32, wz: i32) -> SlabState {
        match self.chunk_at_world(wx, wy, wz) {
            Some((section, lx, ly, lz)) => {
                let block = section.block(lx, ly, lz);
                crate::slab::normalize_state(block, section.slab_state(lx, ly, lz))
            }
            None => SlabState::EMPTY,
        }
    }

    #[inline]
    pub fn slab_state_if_slab(&self, pos: IVec3) -> Option<SlabState> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        crate::slab::is_slab(block).then(|| self.slab_state_at(pos.x, pos.y, pos.z))
    }

    /// The state `pos` would hold after adding one `block` slab layer in `slot` —
    /// the single placement-validity rule, shared by the game's pre-checks (which
    /// need the resulting shape for entity-overlap tests) and the commit in
    /// `place_slab_layer`. `None` when `block` is not a
    /// slab, the cell holds a non-replaceable non-slab block, or the slot is
    /// unavailable (split mismatch / already occupied).
    pub fn slab_layer_target_state(
        &self,
        pos: IVec3,
        block: Block,
        slot: SlabSlot,
    ) -> Option<SlabState> {
        if !crate::slab::is_slab(block) {
            return None;
        }
        let existing_block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        let state = if crate::slab::is_slab(existing_block) {
            self.slab_state_at(pos.x, pos.y, pos.z)
        } else if existing_block.is_replaceable() {
            SlabState::EMPTY
        } else {
            return None;
        };
        crate::slab::add_layer(state, slot, block)
    }
}
