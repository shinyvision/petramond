use crate::chunk::{ChunkPos, SectionPos};

use super::World;

impl World {
    pub(in crate::world) fn remove_section(&mut self, pos: SectionPos) {
        self.terrain.prediction_terrain.cancel_section(pos);
        if let Some(job) = self.terrain.mesh_job_cancels.remove(&pos) {
            job.cancel();
        }
        if let Some(job) = self.gen.pending_section_jobs.remove(&pos) {
            job.cancel();
        }
        self.remove_pending_section(pos);
        let section_removed = self.sections.remove(&pos).is_some();
        if section_removed {
            self.note_section_unloaded(pos);
            self.bump_column_payload_revision(pos.chunk_pos());
        }
        self.block_entity_sections.remove(&pos);
        self.particle_emitter_sections.remove(&pos);
        self.gen.awaited_overlays.remove(&pos);
        self.gen.disk_primary_sections.remove(&pos);
        if self.remove_mesh(pos) {
            self.terrain
                .mesh_upload_dirty_columns
                .insert(pos.chunk_pos());
        }
        self.terrain.dirty_meshes.remove(pos);
        self.terrain.light_blocked_meshes.remove(&pos);
        self.light_deferred.remove(&pos);
        self.deferred_rechecks.remove(&pos);
        self.terrain.deep_sections.remove(&pos);
        self.terrain.visible_deep.remove(&pos);
        self.terrain.hidden_parked.remove(&pos);
        self.terrain.sealed_parked.remove(&pos);
        self.light_bakes.cancel(pos);
        self.light_edited_since_persist.remove(&pos);
        self.evict_custom_bake_section(pos);
        self.mark_light_dirty_neighborhood(pos, false);
        self.mark_dirty_neighborhood(pos, false);
    }

    /// Evict an entire column: all its loaded sections, meshes, queues, per-column data,
    /// and any pending gen.
    pub(in crate::world) fn remove_column(&mut self, pos: ChunkPos) {
        // An evicted column is missing again if an anchor still wants it —
        // the settled short-circuit must not hide it from the next scan.
        self.missing_columns_settled = false;
        let bits = self
            .terrain
            .section_column_cys
            .get(&pos)
            .copied()
            .unwrap_or(0);
        Self::for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.terrain.prediction_terrain.cancel_section(sp);
            self.sections.remove(&sp);
            self.block_entity_sections.remove(&sp);
            self.particle_emitter_sections.remove(&sp);
            self.terrain.meshes.remove(&sp);
            if let Some(job) = self.terrain.mesh_job_cancels.remove(&sp) {
                job.cancel();
            }
            self.terrain.repack_forced.remove(&sp);
            self.terrain.dirty_meshes.remove(sp);
            self.terrain.light_blocked_meshes.remove(&sp);
            self.light_deferred.remove(&sp);
            self.deferred_rechecks.remove(&sp);
            self.terrain.deep_sections.remove(&sp);
            self.terrain.visible_deep.remove(&sp);
            self.terrain.hidden_parked.remove(&sp);
            self.terrain.sealed_parked.remove(&sp);
            self.light_bakes.cancel(sp);
            self.light_edited_since_persist.remove(&sp);
        });
        self.clear_mesh_column_index(pos);
        self.clear_section_column_index(pos);
        self.terrain.mesh_upload_revisions.remove(&pos);
        self.terrain.mesh_upload_dirty_columns.remove(&pos);
        self.terrain.mesh_release_after.remove(&pos);
        self.columns.remove(&pos);
        self.column_payload_revisions.remove(&pos);
        self.gen.column_gen.remove(&pos);
        self.column_summaries.remove(&pos);
        self.column_biome_halos.remove(&pos);
        self.column_deep_band_los.remove(&pos);
        if let Some(Some(job)) = self.gen.pending.remove(&pos) {
            job.cancel();
        }
        let section_jobs: Vec<_> = self
            .gen
            .pending_section_jobs
            .keys()
            .filter(|sp| sp.chunk_pos() == pos)
            .copied()
            .collect();
        for sp in section_jobs {
            if let Some(job) = self.gen.pending_section_jobs.remove(&sp) {
                job.cancel();
            }
        }
        self.clear_pending_sections_for_column(pos);
        self.gen.awaited_overlays.retain(|sp| sp.chunk_pos() != pos);
        self.gen
            .disk_primary_sections
            .retain(|sp| sp.chunk_pos() != pos);
        self.evict_custom_bake_column(pos);
    }

    /// Drop all loaded sections, columns, meshes, and the in-flight gen set — the
    /// regen path.
    pub fn clear_world(&mut self) {
        self.terrain.prediction_terrain.cancel_all();
        self.sections.clear();
        self.terrain.deep_sections.clear();
        self.terrain.visible_deep.clear();
        self.terrain.hidden_parked.clear();
        self.terrain.sealed_parked.clear();
        self.block_entity_sections.clear();
        self.particle_emitter_sections.clear();
        self.columns.clear();
        self.column_payload_revisions.clear();
        self.gen.column_gen.clear();
        self.column_summaries.clear();
        self.column_biome_halos.clear();
        self.column_deep_band_los.clear();
        self.terrain.meshes.clear();
        for job in self.terrain.mesh_job_cancels.values() {
            job.cancel();
        }
        self.terrain.mesh_job_cancels.clear();
        self.terrain.mesh_columns.clear();
        self.terrain.mesh_column_cys.clear();
        self.terrain.section_column_cys.clear();
        self.terrain.section_column_rt.clear();
        self.random_tick_dirty.clear();
        self.terrain.mesh_upload_revisions.clear();
        self.terrain.mesh_upload_dirty_columns.clear();
        self.terrain.mesh_release_after.clear();
        self.terrain.repack_forced.clear();
        self.terrain.light_blocked_meshes.clear();
        self.light_deferred.clear();
        self.light_edited_since_persist.clear();
        self.deferred_recheck_needed = false;
        self.deferred_rechecks.clear();
        for job in self.gen.pending.values().flatten() {
            job.cancel();
        }
        self.gen.pending.clear();
        for job in self.gen.pending_section_jobs.values() {
            job.cancel();
        }
        self.gen.pending_section_jobs.clear();
        self.clear_all_pending_sections();
        self.gen.pending_overlays.clear();
        self.gen.awaited_overlays.clear();
        self.gen.disk_primary_sections.clear();
        self.clear_custom_bake();
        self.bump_terrain_revision();
    }
}
