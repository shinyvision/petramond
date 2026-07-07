//! Chest block-entities at the world level.
//!
//! A chest IS just a generic [`Container`](crate::container::Container) (27
//! plain slots) plus an entity facing for the lidded dynamic render — it has
//! no machine state and doesn't tick. These wrappers own only that pairing:
//! placement installs both, breaking's generic container scatter empties the
//! slots (the facing falls to the generic
//! [`forget_block_entity_records`](super::store::World) sweep), and the
//! render collection walks the facings of chest cells.

use crate::container::Container;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::store::World;

/// A chest's slot count (the classic 3×9 grid).
pub const CHEST_SLOTS: usize = 27;

impl World {
    /// Install an empty chest facing `facing` at a freshly placed chest block.
    /// No-op if the owning chunk is not loaded or `y` is out of range.
    pub fn insert_chest(&mut self, pos: IVec3, facing: Facing) {
        if let Some((c, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            c.insert_container(lx, ly, lz, Container::with_len(CHEST_SLOTS));
            c.insert_entity_facing(lx, ly, lz, facing);
            self.note_block_entity_change(pos);
        }
    }

    /// Append the render data — world position, facing, and sampled light — of
    /// every loaded chest to `out` (cleared first). The transient lid open angle is
    /// filled in by the caller (it's client-side animation, not world state). Visits
    /// only the block-entity section index, not every loaded section.
    pub fn collect_chests(&self, out: &mut Vec<(IVec3, Facing, u8, u8)>) {
        out.clear();
        for sp in &self.block_entity_sections {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            let facings = section.entity_facings();
            if facings.is_empty() {
                continue;
            }
            let (ox, oy, oz) = section.origin_world();
            for (&key, &facing) in facings {
                // Facings are shared by every facing block-entity (furnaces
                // too) — only chest cells get the dynamic chest model.
                // Invert the section-local block index (idx = y*256 + z*16 + x).
                let lx = (key & 0x0F) as usize;
                let lz = ((key >> 4) & 0x0F) as usize;
                let ly = (key >> 8) as usize;
                if section.block(lx, ly, lz) != crate::block::Block::Chest {
                    continue;
                }
                let pos = IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32);
                let sky = self.skylight6_at_world(pos.x, pos.y, pos.z);
                let block = self.blocklight6_at_world(pos.x, pos.y, pos.z);
                out.push((pos, facing, sky, block));
            }
        }
    }
}
