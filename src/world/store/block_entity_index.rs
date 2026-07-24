use crate::chunk::SectionPos;

use super::World;

impl World {
    /// [`refresh_block_entity_index`](Self::refresh_block_entity_index) for the
    /// section owning world cell `pos`.
    pub(in crate::world) fn note_block_entity_change(&mut self, pos: crate::mathh::IVec3) {
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
                // The render fan-outs (door slabs, chest lids) visit this
                // index: a section belongs when any stateful cell is a door
                // or a directional block-entity front.
                || s.cell_states().keys().any(|&idx| {
                    let (lx, ly, lz) = crate::chunk::section_local(idx as usize);
                    let b = s.block(lx, ly, lz);
                    b.shape_family() == crate::block::ShapeFamily::Door || b.directional_view()
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
