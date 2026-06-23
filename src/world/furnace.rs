//! Furnace block-entities at the world level: the per-tick smelting fan-out and
//! world-coordinate access to the chunk-owned furnace maps.
//!
//! Furnaces live on their chunk (see [`crate::chunk::Chunk`]), so these are thin
//! world↔chunk coordinate wrappers plus the tick driver that supplies the recipe
//! set the storage layer is kept ignorant of.

use crate::crafting::Recipes;
use crate::furnace::{Facing, Furnace};
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// Advance every loaded furnace by one game tick, smelting per `recipes`.
    /// Furnaces are chunk-owned, so this just fans out to each chunk, which marks
    /// itself modified (state changed) and mesh-dirty (lit flipped) as needed.
    /// Cheap for the common furnace-free chunk (an empty-map early-out).
    ///
    /// One step of the per-tick sequence owned by [`World::game_tick`]; not a
    /// public entry point.
    pub(super) fn tick_furnaces(&mut self, recipes: &Recipes) {
        for chunk in self.chunks.values_mut() {
            chunk.tick_furnaces(|it| recipes.smelt(it));
        }
    }

    /// The furnace at a world block position, if one is stored there.
    pub fn furnace_at(&self, pos: IVec3) -> Option<&Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.furnace_at(lx, ly, lz)
    }

    /// Mutable handle to the furnace at a world block position (GUI edits).
    pub fn furnace_at_mut(&mut self, pos: IVec3) -> Option<&mut Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.furnace_at_mut(lx, ly, lz)
    }

    /// Install an empty furnace facing `facing` at a freshly placed furnace block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_furnace(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_furnace(
                lx,
                ly,
                lz,
                Furnace {
                    facing,
                    ..Furnace::default()
                },
            );
        }
    }

    /// Remove and return the furnace at a world position (block break), if any.
    pub fn take_furnace(&mut self, pos: IVec3) -> Option<Furnace> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.take_furnace(lx, ly, lz)
    }
}
