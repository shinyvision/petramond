use crate::world::{World, WorldSide};
use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;

pub(in crate::world) fn indexes_block_entity(block: Block) -> bool {
    block.animated_model().is_some() || block.directional_view()
}

impl<S: WorldSide> World<S> {
    pub(in crate::world) fn note_block_entity_change(&mut self, pos: petramond_math::math::IVec3) {
        if let Some(sp) = SectionPos::from_world(pos.x, pos.y, pos.z) {
            self.refresh_block_entity_index(sp);
        }
    }

    pub(in crate::world) fn refresh_block_entity_index(&mut self, pos: SectionPos) {
        let has = self.data.sections.get(&pos).is_some_and(|s| {
            !s.containers().is_empty()
                || !s.furnaces().is_empty()
                || s.cell_states().keys().any(|&idx| {
                    let (lx, ly, lz) = petramond_world::chunk::section_local(idx as usize);
                    indexes_block_entity(s.block(lx, ly, lz))
                })
        });
        if has {
            self.data.block_entity_sections.insert(pos);
        } else {
            self.data.block_entity_sections.remove(&pos);
        }
    }

    pub(in crate::world) fn refresh_presented_index(&mut self, pos: SectionPos) {
        let has = self
            .data
            .sections
            .get(&pos)
            .is_some_and(|s| s.has_presented_cells());
        if has {
            self.data.presented_sections.insert(pos);
        } else {
            self.data.presented_sections.remove(&pos);
        }
    }
}
