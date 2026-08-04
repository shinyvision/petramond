//! Stackable slabs at the world level: position-aware state lookup, collision,
//! and placement.

use petramond_world::block::Block;
use petramond_math::math::IVec3;
use petramond_world::slab::SlabSlot;

use super::store::World;


impl World {
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
        let representative = petramond_world::slab::representative_block(next);
        let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, representative);
        section.set_slab_state(lx, ly, lz, next);
        section.modified = true;
        self.refresh_region(&[pos]);
        true
    }
}
