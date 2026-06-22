//! Furnace block-entities at the world level: the per-tick smelting fan-out and
//! world-coordinate access to the chunk-owned furnace maps.
//!
//! Furnaces live on their chunk (see [`crate::chunk::Chunk`]), so these are thin
//! world↔chunk coordinate wrappers plus the tick driver that supplies the recipe
//! set the storage layer is kept ignorant of.

use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::crafting::Recipes;
use crate::furnace::{Facing, Furnace};
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// Advance every loaded furnace by one game tick, smelting per `recipes`.
    /// Furnaces are chunk-owned, so this just fans out to each chunk, which marks
    /// itself modified (state changed) and mesh-dirty (lit flipped) as needed.
    /// Cheap for the common furnace-free chunk (an empty-map early-out).
    pub fn tick_furnaces(&mut self, recipes: &Recipes) {
        for chunk in self.chunks.values_mut() {
            chunk.tick_furnaces(|it| recipes.smelt(it));
        }
    }

    /// The furnace at a world block position, if one is stored there.
    pub fn furnace_at(&self, pos: IVec3) -> Option<&Furnace> {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return None;
        }
        self.chunks
            .get(&ChunkPos::new(pos.x >> 4, pos.z >> 4))?
            .furnace_at(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
            )
    }

    /// Mutable handle to the furnace at a world block position (GUI edits).
    pub fn furnace_at_mut(&mut self, pos: IVec3) -> Option<&mut Furnace> {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return None;
        }
        self.chunks
            .get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4))?
            .furnace_at_mut(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
            )
    }

    /// Install an empty furnace facing `facing` at a freshly placed furnace block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_furnace(&mut self, pos: IVec3, facing: Facing) {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return;
        }
        if let Some(c) = self.chunks.get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4)) {
            c.insert_furnace(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
                Furnace {
                    facing,
                    ..Furnace::default()
                },
            );
        }
    }

    /// Mark the chunk owning `pos` as modified — called after a GUI edit to a
    /// furnace so the change persists even when the furnace is otherwise idle (and
    /// thus wouldn't be re-marked by the smelting tick).
    pub fn mark_furnace_modified(&mut self, pos: IVec3) {
        if let Some(c) = self.chunks.get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4)) {
            c.modified = true;
        }
    }

    /// Remove and return the furnace at a world position (block break), if any.
    pub fn take_furnace(&mut self, pos: IVec3) -> Option<Furnace> {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return None;
        }
        self.chunks
            .get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4))?
            .take_furnace(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
            )
    }
}
