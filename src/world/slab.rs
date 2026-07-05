//! Stackable slabs at the world level: position-aware state lookup, collision,
//! placement, and drop material recovery.

use crate::block::{Aabb, Block};
use crate::block_state::SlabState;
use crate::item::ItemStack;
use crate::mathh::IVec3;
use crate::slab::SlabSlot;

use super::store::World;

impl World {
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
    pub fn slab_boxes_at(&self, wx: i32, wy: i32, wz: i32) -> &'static [Aabb] {
        crate::slab::boxes_for_state(self.slab_state_at(wx, wy, wz))
    }

    #[inline]
    pub fn slab_visual_aabb_at(&self, wx: i32, wy: i32, wz: i32) -> Option<([f32; 3], [f32; 3])> {
        crate::slab::visual_aabb(self.slab_state_at(wx, wy, wz))
    }

    #[inline]
    pub fn slab_drop_stacks_at(&self, pos: IVec3) -> Vec<ItemStack> {
        crate::slab::drop_stacks(self.slab_state_at(pos.x, pos.y, pos.z))
    }

    #[inline]
    pub fn slab_state_if_slab(&self, pos: IVec3) -> Option<SlabState> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        crate::slab::is_slab(block).then(|| self.slab_state_at(pos.x, pos.y, pos.z))
    }

    /// The state `pos` would hold after adding one `block` slab layer in `slot` —
    /// the single placement-validity rule, shared by the game's pre-checks (which
    /// need the resulting shape for entity-overlap tests) and the commit in
    /// [`place_slab_layer`](Self::place_slab_layer). `None` when `block` is not a
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

    /// Place one slab layer into `pos`, either creating a new slab cell or filling the
    /// empty matching half of an existing slab cell. The caller owns entity-overlap
    /// checks and inventory consumption.
    pub fn place_slab_layer(&mut self, pos: IVec3, block: Block, slot: SlabSlot) -> bool {
        if !self.materialize_section_at(pos) {
            return false;
        }
        let Some(next) = self.slab_layer_target_state(pos, block, slot) else {
            return false;
        };
        let representative = crate::slab::representative_block(next);
        let Some((section_pos, _, _, _)) = Self::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, representative);
        section.set_slab_state(lx, ly, lz, next);
        section.modified = true;
        if self.update_column_height_after_set(pos.x, pos.y, pos.z, true) {
            self.mark_heightmap_light_dirty_around(section_pos.chunk_pos());
        }
        self.refresh_region(&[pos]);
        true
    }
}
