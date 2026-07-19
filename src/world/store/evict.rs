use crate::chunk::{ChunkPos, SectionPos};

use super::World;

impl World {
    pub(in crate::world) fn remove_section(&mut self, pos: SectionPos) {
        self.prediction_terrain.cancel_section(pos);
        if let Some(job) = self.mesh_job_cancels.remove(&pos) {
            job.cancel();
        }
        if let Some(job) = self.pending_section_jobs.remove(&pos) {
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
        self.awaited_overlays.remove(&pos);
        self.disk_primary_sections.remove(&pos);
        if self.remove_mesh(pos) {
            self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
        }
        self.dirty_meshes.remove(pos);
        self.light_blocked_meshes.remove(&pos);
        self.light_deferred.remove(&pos);
        self.deferred_rechecks.remove(&pos);
        self.deep_sections.remove(&pos);
        self.visible_deep.remove(&pos);
        self.hidden_parked.remove(&pos);
        self.sealed_parked.remove(&pos);
        self.light_bakes.cancel(pos);
        self.light_edited_since_persist.remove(&pos);
        self.mark_light_dirty_neighborhood(pos, false);
        self.mark_dirty_neighborhood(pos, false);
    }

    /// Evict an entire column: all its loaded sections, meshes, queues, per-column data,
    /// and any pending gen.
    pub(in crate::world) fn remove_column(&mut self, pos: ChunkPos) {
        // An evicted column is missing again if an anchor still wants it —
        // the settled short-circuit must not hide it from the next scan.
        self.missing_columns_settled = false;
        let bits = self.section_column_cys.get(&pos).copied().unwrap_or(0);
        Self::for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.prediction_terrain.cancel_section(sp);
            self.sections.remove(&sp);
            self.block_entity_sections.remove(&sp);
            self.particle_emitter_sections.remove(&sp);
            self.meshes.remove(&sp);
            if let Some(job) = self.mesh_job_cancels.remove(&sp) {
                job.cancel();
            }
            self.repack_forced.remove(&sp);
            self.dirty_meshes.remove(sp);
            self.light_blocked_meshes.remove(&sp);
            self.light_deferred.remove(&sp);
            self.deferred_rechecks.remove(&sp);
            self.deep_sections.remove(&sp);
            self.visible_deep.remove(&sp);
            self.hidden_parked.remove(&sp);
            self.sealed_parked.remove(&sp);
            self.light_bakes.cancel(sp);
            self.light_edited_since_persist.remove(&sp);
        });
        self.clear_mesh_column_index(pos);
        self.clear_section_column_index(pos);
        self.mesh_upload_revisions.remove(&pos);
        self.mesh_upload_dirty_columns.remove(&pos);
        self.mesh_release_after.remove(&pos);
        self.columns.remove(&pos);
        self.column_payload_revisions.remove(&pos);
        self.column_gen.remove(&pos);
        self.column_summaries.remove(&pos);
        self.column_biome_halos.remove(&pos);
        self.column_deep_band_los.remove(&pos);
        if let Some(Some(job)) = self.pending.remove(&pos) {
            job.cancel();
        }
        let section_jobs: Vec<_> = self
            .pending_section_jobs
            .keys()
            .filter(|sp| sp.chunk_pos() == pos)
            .copied()
            .collect();
        for sp in section_jobs {
            if let Some(job) = self.pending_section_jobs.remove(&sp) {
                job.cancel();
            }
        }
        self.clear_pending_sections_for_column(pos);
        self.awaited_overlays.retain(|sp| sp.chunk_pos() != pos);
        self.disk_primary_sections
            .retain(|sp| sp.chunk_pos() != pos);
    }

    /// Drop all loaded sections, columns, meshes, and the in-flight gen set — the
    /// regen path.
    pub fn clear_world(&mut self) {
        self.prediction_terrain.cancel_all();
        self.sections.clear();
        self.deep_sections.clear();
        self.visible_deep.clear();
        self.hidden_parked.clear();
        self.sealed_parked.clear();
        self.block_entity_sections.clear();
        self.particle_emitter_sections.clear();
        self.columns.clear();
        self.column_payload_revisions.clear();
        self.column_gen.clear();
        self.column_summaries.clear();
        self.column_biome_halos.clear();
        self.column_deep_band_los.clear();
        self.meshes.clear();
        for job in self.mesh_job_cancels.values() {
            job.cancel();
        }
        self.mesh_job_cancels.clear();
        self.mesh_columns.clear();
        self.mesh_column_cys.clear();
        self.section_column_cys.clear();
        self.mesh_upload_revisions.clear();
        self.mesh_upload_dirty_columns.clear();
        self.mesh_release_after.clear();
        self.repack_forced.clear();
        self.light_blocked_meshes.clear();
        self.light_deferred.clear();
        self.light_edited_since_persist.clear();
        self.deferred_recheck_needed = false;
        self.deferred_rechecks.clear();
        for job in self.pending.values().flatten() {
            job.cancel();
        }
        self.pending.clear();
        for job in self.pending_section_jobs.values() {
            job.cancel();
        }
        self.pending_section_jobs.clear();
        self.clear_all_pending_sections();
        self.pending_overlays.clear();
        self.awaited_overlays.clear();
        self.disk_primary_sections.clear();
        self.bump_terrain_revision();
    }
}
