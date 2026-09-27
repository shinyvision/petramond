use crate::world::{World, WorldSide};
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::for_each_column_cy;

impl<S: WorldSide> World<S> {
    pub(in crate::world) fn remove_section(&mut self, pos: SectionPos) {
        if let Some(replica) = self.side.replica_mut() {
            replica.terrain.forget_section(pos);
        }
        if let Some(server) = self.side.server_mut() {
            if let Some(job) = server.gen.pending_section_jobs.remove(&pos) {
                job.cancel();
                server.worker.remove_queued([job.ticket]);
            }
            server.gen.awaited_overlays.remove(&pos);
            server.gen.disk_primary_sections.remove(&pos);
        }
        self.forget_stream_section(pos);
        let section_removed = self.data.sections.remove(&pos).is_some();
        if section_removed {
            self.unstamp_section(pos);
            self.note_section_unloaded(pos);
            self.data.bump_column_payload_revision(pos.chunk_pos());
        }
        self.data.block_entity_sections.remove(&pos);
        if section_removed {
            self.forget_block_draws_in_section(pos);
        }
        self.data.particle_emitter_sections.remove(&pos);
        self.forget_section_mesh(pos);
        self.data.light_deferred.remove(&pos);
        self.data.deferred_rechecks.remove(&pos);
        self.light_bakes.cancel(pos);
        self.data.light_edited_since_persist.remove(&pos);
        self.data.evict_custom_bake_section(pos);
        self.mark_light_dirty_neighborhood(pos, false);
        self.mark_dirty_neighborhood(pos, false);
    }

    pub(in crate::world) fn remove_column(&mut self, pos: ChunkPos) {
        self.data.missing_columns_settled = false;
        let bits = self.data.section_column_cys.get(&pos).copied().unwrap_or(0);
        self.unstamp_column(pos, bits);
        for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.forget_block_draws_in_section(sp);
            if let Some(replica) = self.side.replica_mut() {
                replica.terrain.meshes.remove(&sp);
                replica.terrain.forget_section(sp);
            }
            self.data.sections.remove(&sp);
            self.data.block_entity_sections.remove(&sp);
            self.data.particle_emitter_sections.remove(&sp);
            self.data.light_deferred.remove(&sp);
            self.data.deferred_rechecks.remove(&sp);
            self.light_bakes.cancel(sp);
            self.data.light_edited_since_persist.remove(&sp);
        });
        if let Some(replica) = self.side.replica_mut() {
            let terrain = &mut replica.terrain;
            terrain.mesh_columns.remove(&pos);
            terrain.mesh_column_cys.remove(&pos);
            terrain.mesh_upload_revisions.remove(&pos);
            terrain.mesh_upload_dirty_columns.remove(&pos);
            terrain.mesh_release_after.remove(&pos);
        }
        self.clear_section_column_index(pos);
        self.data.columns.remove(&pos);
        self.data.column_payload_revisions.remove(&pos);
        self.data.column_summaries.remove(&pos);
        self.data.column_biome_halos.remove(&pos);
        self.data.column_deep_band_los.remove(&pos);
        if let Some(server) = self.side.server_mut() {
            let gen = &mut server.gen;
            gen.column_gen.remove(&pos);
            if let Some(Some(job)) = gen.pending.remove(&pos) {
                job.cancel();
            }
            let mut evicted_jobs = Vec::new();
            gen.pending_section_jobs.retain(|sp, job| {
                let keep = sp.chunk_pos() != pos;
                if !keep {
                    job.cancel();
                    evicted_jobs.push(job.ticket);
                }
                keep
            });
            server.worker.remove_queued(evicted_jobs);
        }
        self.forget_stream_column(pos);
        if let Some(server) = self.side.server_mut() {
            let gen = &mut server.gen;
            gen.awaited_overlays.retain(|sp| sp.chunk_pos() != pos);
            gen.disk_primary_sections.retain(|sp| sp.chunk_pos() != pos);
        }
        self.data.evict_custom_bake_column(pos);
    }

    pub fn clear_world(&mut self) {
        if let Some(replica) = self.side.replica_mut() {
            replica.terrain.clear();
        }
        self.data.sections.clear();
        self.data.block_entity_sections.clear();
        self.draws.block_draws.clear();
        self.draws.block_draw_sections.clear();
        self.data.particle_emitter_sections.clear();
        self.data.columns.clear();
        self.data.column_payload_revisions.clear();
        self.data.column_summaries.clear();
        self.data.column_biome_halos.clear();
        self.data.column_deep_band_los.clear();
        self.data.section_column_cys.clear();
        self.data.section_column_rt.clear();
        self.data.random_tick_dirty.clear();
        self.data.light_deferred.clear();
        self.data.light_edited_since_persist.clear();
        self.data.deferred_recheck_needed = false;
        self.data.deferred_rechecks.clear();
        if let Some(server) = self.side.server_mut() {
            let gen = &mut server.gen;
            gen.column_gen.clear();
            for job in gen.pending.values().flatten() {
                job.cancel();
            }
            gen.pending.clear();
            for job in gen.pending_section_jobs.values() {
                job.cancel();
            }
            server
                .worker
                .remove_queued(gen.pending_section_jobs.values().map(|job| job.ticket));
            gen.pending_section_jobs.clear();
            gen.pending_sections.clear();
            gen.pending_section_columns.clear();
            gen.section_requests_unsettled = false;
            gen.pending_overlays.clear();
            gen.awaited_overlays.clear();
            gen.disk_primary_sections.clear();
        }
        self.data.stream_nonfinal.clear();
        self.data.clear_custom_bake();
        self.bump_terrain_revision();
    }

    fn forget_stream_section(&mut self, pos: SectionPos) {
        let Some(server) = self.side.server_mut() else {
            return;
        };
        let gen = &mut server.gen;
        if gen.pending_sections.remove(&pos) {
            let column = pos.chunk_pos();
            if let Some(count) = gen.pending_section_columns.get_mut(&column) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    gen.pending_section_columns.remove(&column);
                }
            }
        }
        if !gen.pending_sections.contains(&pos)
            && !gen.awaited_overlays.contains(&pos)
            && !gen.pending_overlays.contains_key(&pos)
        {
            self.data.stream_nonfinal.remove(&pos);
        }
    }

    fn forget_stream_column(&mut self, pos: ChunkPos) {
        let Some(server) = self.side.server_mut() else {
            return;
        };
        let gen = &mut server.gen;
        gen.pending_sections.retain(|sp| sp.chunk_pos() != pos);
        gen.pending_section_columns.remove(&pos);
        self.data.stream_nonfinal = gen
            .pending_sections
            .iter()
            .chain(gen.awaited_overlays.iter())
            .chain(gen.pending_overlays.keys())
            .copied()
            .collect();
    }

    fn forget_section_mesh(&mut self, pos: SectionPos) {
        if let Some(replica) = self.side.replica_mut() {
            if replica.terrain.remove_mesh(pos) {
                replica
                    .terrain
                    .mesh_upload_dirty_columns
                    .insert(pos.chunk_pos());
            }
        }
    }
}
