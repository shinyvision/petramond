use crate::world::{World, WorldSide};
use petramond_math::facing::Facing;
use petramond_math::math::IVec3;
use petramond_world::container::Container;

pub const CHEST_SLOTS: usize = 27;

impl<S: WorldSide> World<S> {
    pub fn insert_chest(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_container(lx, ly, lz, Container::with_len(CHEST_SLOTS));
            c.insert_entity_facing(lx, ly, lz, facing);
            self.note_block_entity_change(pos);
        }
    }

    pub fn insert_entity_facing(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_entity_facing(lx, ly, lz, facing);
            self.note_block_entity_change(pos);
        }
    }
}
