use petramond_world::block::{Block, ShapeFamily};
use petramond_world::chunk::SectionPos;

use super::World;

/// Whether a cell holding `block` puts its section in the block-entity index:
/// the render fan-outs that draw OUTSIDE the chunk mesh (hinged panels, chest
/// lids) walk that index, so a block they draw must be admitted by it. Shared
/// by the index refresh and by every writer that must notice a fresh one —
/// a panel missing from the index is simply invisible.
pub(in crate::world) fn indexes_block_entity(block: Block) -> bool {
    matches!(
        block.shape_family(),
        ShapeFamily::Door | ShapeFamily::Trapdoor
    ) || block.directional_view()
}

impl World {
    /// [`refresh_block_entity_index`](Self::refresh_block_entity_index) for the
    /// section owning world cell `pos`.
    pub(in crate::world) fn note_block_entity_change(&mut self, pos: petramond_math::math::IVec3) {
        if let Some(sp) = SectionPos::from_world(pos.x, pos.y, pos.z) {
            self.refresh_block_entity_index(sp);
        }
    }

    /// Keep [`block_entity_sections`](Self::block_entity_sections) in sync after
    /// `pos`'s content may have changed (section install, container/door/furnace
    /// insert or removal).
    pub(in crate::world) fn refresh_block_entity_index(&mut self, pos: SectionPos) {
        let has = self.sections.get(&pos).is_some_and(|s| {
            !s.containers().is_empty()
                || !s.furnaces().is_empty()
                || s.cell_states().keys().any(|&idx| {
                    let (lx, ly, lz) = petramond_world::chunk::section_local(idx as usize);
                    indexes_block_entity(s.block(lx, ly, lz))
                })
        });
        if has {
            self.block_entity_sections.insert(pos);
        } else {
            self.block_entity_sections.remove(&pos);
        }
    }

    /// Keep [`particle_emitter_sections`](Self::particle_emitter_sections) in sync after
    /// `pos`'s block ids may have changed.
    pub(in crate::world) fn refresh_particle_emitter_index(&mut self, pos: SectionPos) {
        let has = self
            .sections
            .get(&pos)
            .is_some_and(|s| s.has_particle_emitters());
        if has {
            self.particle_emitter_sections.insert(pos);
        } else {
            self.particle_emitter_sections.remove(&pos);
        }
    }
}
