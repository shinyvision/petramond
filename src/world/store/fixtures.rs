use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY};
use crate::section::{Section, SectionSummary};

use super::{LoadTarget, World};

impl World {
    /// Install a section for a test, mirroring the streamer's per-section install.
    #[cfg(test)]
    pub(crate) fn insert_section_for_test(&mut self, pos: SectionPos, section: Section) {
        self.ensure_column(pos.chunk_pos());
        self.sections.insert(pos, Arc::new(section));
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.queue_dirty_mesh(pos);
        self.request_fixture_bake(pos);
        self.bump_terrain_revision();
    }

    /// Fixture sections bypass the streamer's settle/defer path, so a headless
    /// world would never bake them — and the light-final ship gate would hold
    /// them back forever. Feed the relight queue like an edit does.
    #[cfg(test)]
    fn request_fixture_bake(&mut self, pos: SectionPos) {
        self.relight_demand.insert(pos);
    }

    /// Install a whole column [`Chunk`] for a test, splitting it into sections + column
    /// data exactly as the streamer does for a generated column. Lets the many column-era
    /// fixtures (which build a 256-tall `Chunk` and hand it over) keep working against the
    /// cubic store unchanged. `pos` must match the chunk's own `(cx,cz)`.
    ///
    /// Carries blocks + water + biome + heightmap, but NOT block-entities (furnaces,
    /// chests, …) — matching real worldgen, which produces none. A test that needs a
    /// block-entity should build the [`Section`] directly (with `insert_section_for_test`)
    /// or place it through the world API after install.
    #[cfg(test)]
    pub(crate) fn insert_chunk_for_test(&mut self, pos: ChunkPos, chunk: crate::chunk::Chunk) {
        debug_assert_eq!((pos.cx, pos.cz), (chunk.cx, chunk.cz));
        let (column, sections) = crate::world::stream::split_generated_column(&chunk);
        self.columns.insert(pos, column);
        // A test chunk is fully known, so record per-section summaries: its
        // absent sections are genuinely empty sky, and probes that consult
        // `section_summary` (sapling growth validation, physics) read them as
        // air — matching what a generated column's facts would answer.
        let mut sums = vec![SectionSummary::Empty; (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize]
            .into_boxed_slice();
        for (cy, section) in sections {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            sums[(cy - SECTION_MIN_CY) as usize] = section.summary();
            self.sections.insert(sp, Arc::new(section));
            self.refresh_particle_emitter_index(sp);
            self.queue_dirty_mesh(sp);
            self.request_fixture_bake(sp);
        }
        self.column_summaries.insert(pos, sums);
        self.bump_terrain_revision();
    }

    /// Arm the random-tick / streaming anchor for a test world. Random ticks
    /// only run around a load target, which production code sets from the
    /// per-frame streaming path a direct-stepping test never exercises.
    #[cfg(test)]
    pub(crate) fn set_load_target_for_test(&mut self, cx: i32, cy: i32, cz: i32, render_dist: i32) {
        self.last_load_target = Some(LoadTarget::new(cx, cy, cz, render_dist));
    }

    /// Install an entire column of empty (all-air) sections for a test, so
    /// world-coordinate edits anywhere in the vertical range land in a loaded section.
    /// Unlike [`insert_chunk_for_test`](Self::insert_chunk_for_test) with an empty
    /// `Chunk` (whose all-air surface sections would be skipped), this keeps every
    /// section present — the cubic analogue of the column era's "one empty loaded chunk".
    #[cfg(test)]
    pub(crate) fn insert_empty_column_for_test(&mut self, pos: ChunkPos) {
        self.ensure_column(pos);
        for cy in Self::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.sections
                .insert(sp, Arc::new(Section::new(pos.cx, cy, pos.cz)));
            self.refresh_particle_emitter_index(sp);
            self.queue_dirty_mesh(sp);
            self.request_fixture_bake(sp);
        }
        self.bump_terrain_revision();
    }

    /// The loaded section owning world voxel `(wx,wy,wz)`, for a test that inspects
    /// per-section light/flags after a world-coordinate edit.
    #[cfg(test)]
    pub(crate) fn section_at_world_for_test(&self, wx: i32, wy: i32, wz: i32) -> Option<&Section> {
        let pos = SectionPos::from_world(wx, wy, wz)?;
        self.sections.get(&pos).map(|s| &**s)
    }

    /// Mutable counterpart of [`section_at_world_for_test`](Self::section_at_world_for_test).
    #[cfg(test)]
    pub(crate) fn section_at_world_mut_for_test(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<&mut Section> {
        let pos = SectionPos::from_world(wx, wy, wz)?;
        self.section_mut(pos)
    }
}
