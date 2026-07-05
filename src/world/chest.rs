//! Chest block-entities at the world level: world-coordinate access to the
//! chunk-owned chest maps.
//!
//! Chests live on their chunk (see [`crate::chunk::Chunk`]) and don't tick (no
//! smelting), so — unlike [`world::furnace`](super::furnace) — there is no per-tick
//! driver here, just thin world↔chunk coordinate wrappers for placement, GUI edits,
//! and breaking. Mirrors the furnace wrappers minus `tick_furnaces`.

use crate::chest::Chest;
use crate::furnace::Facing;
use crate::mathh::IVec3;

use super::store::World;

impl World {
    /// The chest at a world block position, if one is stored there.
    pub fn chest_at(&self, pos: IVec3) -> Option<&Chest> {
        let (c, lx, ly, lz) = self.chunk_at_world(pos.x, pos.y, pos.z)?;
        c.chest_at(lx, ly, lz)
    }

    /// Mutable handle to the chest at a world block position (GUI edits).
    pub fn chest_at_mut(&mut self, pos: IVec3) -> Option<&mut Chest> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        c.chest_at_mut(lx, ly, lz)
    }

    /// Install an empty chest facing `facing` at a freshly placed chest block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_chest(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_chest(
                lx,
                ly,
                lz,
                Chest {
                    facing,
                    ..Chest::default()
                },
            );
            self.note_block_entity_change(pos);
        }
    }

    /// Remove and return the chest at a world position (block break), if any.
    pub fn take_chest(&mut self, pos: IVec3) -> Option<Chest> {
        let (c, lx, ly, lz) = self.chunk_at_world_mut(pos.x, pos.y, pos.z)?;
        let chest = c.take_chest(lx, ly, lz);
        self.note_block_entity_change(pos);
        chest
    }

    /// Append the render data — world position, facing, and sampled skylight — of
    /// every loaded chest to `out` (cleared first). The transient lid open angle is
    /// filled in by the caller (it's client-side animation, not world state). Visits
    /// only the block-entity section index, not every loaded section.
    pub fn collect_chests(&self, out: &mut Vec<(IVec3, Facing, u8, u8)>) {
        out.clear();
        for sp in &self.block_entity_sections {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            let chests = section.chests();
            if chests.is_empty() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
            for (&key, chest) in chests {
                // Invert the section-local block index (idx = y*256 + z*16 + x).
                let lx = (key & 0x0F) as i32;
                let lz = ((key >> 4) & 0x0F) as i32;
                let ly = (key >> 8) as i32;
                let pos = IVec3::new(ox + lx, oy + ly, oz + lz);
                let sky = self.skylight6_at_world(pos.x, pos.y, pos.z);
                let block = self.blocklight6_at_world(pos.x, pos.y, pos.z);
                out.push((pos, chest.facing, sky, block));
            }
        }
    }
}
