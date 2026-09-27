use crate::block::Block;
use crate::block_state::SlabState;
use crate::mathh::IVec3;
use crate::slab::SlabSlot;

use super::data::WorldData;

impl WorldData {
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
