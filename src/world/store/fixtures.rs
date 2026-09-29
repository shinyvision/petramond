use crate::world::WorldData;
use crate::world::{ReplicaWorld, ServerWorld, World, WorldSide};
use std::sync::Arc;

use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY};
use petramond_world::section::{Section, SectionSummary};

impl<S: WorldSide> World<S> {
    #[cfg(any(test, feature = "test-support"))]
    pub fn clear_update_notifications_for_test(&mut self) {
        self.data.sim.update_queue.clear();
        self.data.sim.update_set.clear();
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_water_source_world(&self, pos: petramond_math::math::IVec3) -> bool {
        self.data
            .is_fluid_source_world(pos, petramond_world::block::Block::Water)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_section_for_test(&mut self, pos: SectionPos, section: Section) {
        self.data.ensure_column(pos.chunk_pos());
        self.data.sections.insert(pos, Arc::new(section));
        self.note_section_loaded(pos);
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.queue_dirty_mesh(pos);
        self.request_fixture_bake(pos);
        self.bump_terrain_revision();
    }

    #[cfg(any(test, feature = "test-support"))]
    fn request_fixture_bake(&mut self, pos: SectionPos) {
        self.data.relight_demand.insert(pos);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_chunk_for_test(&mut self, pos: ChunkPos, chunk: petramond_world::chunk::Chunk) {
        debug_assert_eq!((pos.cx, pos.cz), (chunk.cx, chunk.cz));
        let (column, sections) = crate::world::stream::split_generated_column(&chunk);
        self.data.columns.insert(pos, std::sync::Arc::new(column));
        let mut sums = vec![SectionSummary::Empty; (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize]
            .into_boxed_slice();
        for (cy, section) in sections {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            sums[(cy - SECTION_MIN_CY) as usize] = section.summary();
            self.data.sections.insert(sp, Arc::new(section));
            self.note_section_loaded(sp);
            self.refresh_particle_emitter_index(sp);
            self.queue_dirty_mesh(sp);
            self.request_fixture_bake(sp);
        }
        self.data.column_summaries.insert(pos, sums);
        self.bump_terrain_revision();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_empty_column_for_test(&mut self, pos: ChunkPos) {
        self.data.ensure_column(pos);
        for cy in WorldData::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.data
                .sections
                .insert(sp, Arc::new(Section::new(pos.cx, cy, pos.cz)));
            self.note_section_loaded(sp);
            self.refresh_particle_emitter_index(sp);
            self.queue_dirty_mesh(sp);
            self.request_fixture_bake(sp);
        }
        self.bump_terrain_revision();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn section_at_world_for_test(&self, wx: i32, wy: i32, wz: i32) -> Option<&Section> {
        let pos = SectionPos::from_world(wx, wy, wz)?;
        self.data.sections.get(&pos).map(|s| &**s)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn section_at_world_mut_for_test(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<&mut Section> {
        let pos = SectionPos::from_world(wx, wy, wz)?;
        self.data.section_mut(pos)
    }
}

impl ServerWorld {
    #[cfg(any(test, feature = "test-support"))]
    pub fn mark_overlay_in_flight_for_test(&mut self, pos: SectionPos) {
        self.side.gen.awaited_overlays.insert(pos);
        self.note_stream_nonfinal(pos);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn settle_overlay_for_test(&mut self, pos: SectionPos) {
        self.side.gen.awaited_overlays.remove(&pos);
        self.settle_stream_nonfinal(pos);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn evict_section_for_test(&mut self, pos: SectionPos) {
        self.remove_section(pos);
        self.bump_terrain_revision();
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn evict_column_for_test(&mut self, pos: ChunkPos) {
        self.remove_column(pos);
        self.bump_terrain_revision();
    }
}

impl ReplicaWorld {
    #[cfg(any(test, feature = "test-support"))]
    pub fn mark_overlay_in_flight_for_test(&mut self, pos: SectionPos) {
        self.data.stream_nonfinal.insert(pos);
    }
}
