//! Chest block-entities at the world level.
//!
//! A chest IS just a generic [`Container`] (27
//! plain slots) plus an entity facing for the lidded dynamic render — it has
//! no machine state and doesn't tick. These wrappers own only that pairing:
//! placement installs both, breaking's generic container scatter empties the
//! slots (the facing falls to the generic
//! [`forget_block_entity_records`](super::store::World) sweep), and the
//! lidded model is gathered like every animated block
//! ([`collect_animated_blocks`](super::store::World::collect_animated_blocks)).

use crate::world::{World, WorldSide};
use petramond_math::facing::Facing;
use petramond_math::math::IVec3;
use petramond_world::container::Container;


/// A chest's slot count (the classic 3×9 grid).
pub const CHEST_SLOTS: usize = 27;

impl<S: WorldSide> World<S> {
    /// Install an empty chest facing `facing` at a freshly placed chest block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_chest(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_container(lx, ly, lz, Container::with_len(CHEST_SLOTS));
            c.insert_entity_facing(lx, ly, lz, facing);
            self.note_block_entity_change(pos);
        }
    }

    /// Record just the front facing for a freshly placed directional block
    /// (chest, furnace) — the replica-side mirror of the server's placement
    /// state write, WITHOUT fabricating a local block-entity: containers and
    /// furnace machine state are server-owned and arrive with the delta. The
    /// index refresh makes the animated-block gather collect the cell at once.
    pub fn insert_entity_facing(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_entity_facing(lx, ly, lz, facing);
            self.note_block_entity_change(pos);
        }
    }
}
