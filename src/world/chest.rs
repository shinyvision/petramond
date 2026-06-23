//! Chest block-entities at the world level: world-coordinate access to the
//! chunk-owned chest maps.
//!
//! Chests live on their chunk (see [`crate::chunk::Chunk`]) and don't tick (no
//! smelting), so — unlike [`world::furnace`](super::furnace) — there is no per-tick
//! driver here, just thin world↔chunk coordinate wrappers for placement, GUI edits,
//! and breaking. Mirrors the furnace wrappers minus `tick_furnaces`.

use crate::chest::Chest;
use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::furnace::Facing;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The chest at a world block position, if one is stored there.
    pub fn chest_at(&self, pos: IVec3) -> Option<&Chest> {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return None;
        }
        self.chunks
            .get(&ChunkPos::new(pos.x >> 4, pos.z >> 4))?
            .chest_at(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
            )
    }

    /// Mutable handle to the chest at a world block position (GUI edits).
    pub fn chest_at_mut(&mut self, pos: IVec3) -> Option<&mut Chest> {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return None;
        }
        self.chunks
            .get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4))?
            .chest_at_mut(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
            )
    }

    /// Install an empty chest facing `facing` at a freshly placed chest block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_chest(&mut self, pos: IVec3, facing: Facing) {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return;
        }
        if let Some(c) = self.chunks.get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4)) {
            c.insert_chest(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
                Chest {
                    facing,
                    ..Chest::default()
                },
            );
        }
    }

    /// Mark the chunk owning `pos` as modified — called after a GUI edit to a chest
    /// so the change persists even when the chest is otherwise untouched by any tick.
    pub fn mark_chest_modified(&mut self, pos: IVec3) {
        if let Some(c) = self.chunks.get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4)) {
            c.modified = true;
        }
    }

    /// Remove and return the chest at a world position (block break), if any.
    pub fn take_chest(&mut self, pos: IVec3) -> Option<Chest> {
        if pos.y < 0 || pos.y >= CHUNK_SY as i32 {
            return None;
        }
        self.chunks
            .get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4))?
            .take_chest(
                (pos.x & 0x0F) as usize,
                pos.y as usize,
                (pos.z & 0x0F) as usize,
            )
    }

    /// Append the render data — world position, facing, and sampled skylight — of
    /// every loaded chest to `out` (cleared first). The transient lid open angle is
    /// filled in by the caller (it's client-side animation, not world state). Cheap
    /// for the common chest-free world: each chunk early-outs on an empty chest map.
    pub fn collect_chests(&self, out: &mut Vec<(IVec3, Facing, u8)>) {
        out.clear();
        for chunk in self.chunks.values() {
            let chests = chunk.chests();
            if chests.is_empty() {
                continue;
            }
            let (ox, oz) = chunk.chunk_origin_world();
            for (&key, chest) in chests {
                // Invert the local block index (idx = y*256 + z*16 + x).
                let lx = (key & 0x0F) as i32;
                let lz = ((key >> 4) & 0x0F) as i32;
                let ly = (key >> 8) as i32;
                let pos = IVec3::new(ox + lx, ly, oz + lz);
                let sky = self.skylight6_at_world(pos.x, pos.y, pos.z);
                out.push((pos, chest.facing, sky));
            }
        }
    }
}
