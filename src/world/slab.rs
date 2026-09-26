//! Stackable slabs at the world level: position-aware state lookup, collision,
//! and placement.

use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::slab::SlabSlot;

use super::cell_change::{CellChange, ChangeKind};


impl<S: WorldSide> World<S> {
    /// Place one slab layer into `pos`, either creating a new slab cell or filling the
    /// empty matching half of an existing slab cell. The caller owns entity-overlap
    /// checks and inventory consumption.
    pub fn place_slab_layer(&mut self, pos: IVec3, block: Block, slot: SlabSlot) -> bool {
        if !self.materialize_section_at(pos) {
            return false;
        }
        let Some(next) = self.data.slab_layer_target_state(pos, block, slot) else {
            return false;
        };
        let representative = petramond_world::slab::representative_block(next);
        let Some((section, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        let old = section.block(lx, ly, lz);
        section.set_block(lx, ly, lz, representative);
        section.set_slab_state(lx, ly, lz, next);
        section.modified = true;
        self.apply_cell_changes(&[CellChange::new(pos, old, ChangeKind::Place)]);
        true
    }
}
