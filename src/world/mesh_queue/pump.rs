use crate::world::ReplicaWorld;
#[cfg(test)]
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::{
    max_mesh_jobs_in_flight, CANDIDATE_SCAN_PER_MESH_JOB, MESH_SUBMIT_TIME_BUDGET,
    MIN_MESH_JOBS_PER_PUMP,
};

impl ReplicaWorld {
    pub fn tick_mesh_budget(&mut self, max_per_frame: usize) {
        self.side.terrain.mesh_pump_frame += 1;
        self.side.terrain.mesh_pump_now = std::time::Instant::now();
        self.drain_prediction_terrain();
        self.pump_light_bakes();
        self.drain_finished_meshes();
        self.release_settled_column_meshes();
        if max_per_frame == 0 {
            return;
        }
        let submit_start = std::time::Instant::now();

        let in_flight_room =
            max_mesh_jobs_in_flight().saturating_sub(self.side.terrain.mesh_jobs_in_flight);
        if in_flight_room == 0 {
            return;
        }
        let target_jobs = max_per_frame
            .max(MIN_MESH_JOBS_PER_PUMP)
            .min(in_flight_room);
        let candidate_cap = target_jobs.saturating_mul(CANDIDATE_SCAN_PER_MESH_JOB);
        if self.side.terrain.vis_dirty && self.side.terrain.mesh_pump_frame.is_multiple_of(8) {
            self.refresh_deep_visibility();
        }
        if submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
            return;
        }
        let target = self.data.last_load_target;
        let candidates = self
            .side
            .terrain
            .dirty_meshes
            .pop_nearest_batch(candidate_cap, target);
        let mut submitted = 0usize;
        for (i, &pos) in candidates.iter().enumerate() {
            if submitted > 0 && submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                for &rest in &candidates[i..] {
                    self.side.terrain.dirty_meshes.push(rest);
                }
                break;
            }
            let nbhd = self.gather_mesh_neighbourhood(pos);
            let Some(center) = nbhd[crate::world::mesh_pool::nbhd_idx27(0, 0, 0)].as_ref() else {
                continue;
            };
            let center_is_air = center.is_empty_air();
            if self.side.terrain.prediction_terrain.owns_mesh(pos) {
                self.side.terrain.light_blocked_meshes.insert(pos);
                continue;
            }
            // Nothing can see this section, so park it out of the hot queue. Light parks with it
            // (light is mesh-demanded). The visibility refresh re-queues it once a sightline
            // reaches it. Repack-forced sections skip this: their released geometry is still in the
            // packed column buffer, so the repack needs a fresh mesh even while hidden.
            if self.section_hidden(pos) && !self.side.terrain.repack_forced.contains(&pos) {
                self.side.terrain.hidden_parked.insert(pos);
                continue;
            }
            if center_is_air {
                self.settle_no_mesh_output(pos);
                if submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                    for &rest in &candidates[i + 1..] {
                        self.side.terrain.dirty_meshes.push(rest);
                    }
                    break;
                }
                continue;
            }
            if self.sealed_by_loaded_neighbors_in(pos, &nbhd)
                && !self.side.terrain.repack_forced.contains(&pos)
            {
                self.side.terrain.sealed_parked.insert(pos);
                self.side.terrain.dirty_meshes.remove(pos);
                self.side.terrain.light_blocked_meshes.remove(&pos);
                if let Some(s) = self.data.section_mut(pos) {
                    s.dirty = false;
                    s.mesh_revision = s.mesh_revision.wrapping_add(1);
                }
                continue;
            }
            if self.request_light_dependencies(pos, &nbhd) {
                self.side.terrain.light_blocked_meshes.insert(pos);
                if submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                    for &rest in &candidates[i + 1..] {
                        self.side.terrain.dirty_meshes.push(rest);
                    }
                    break;
                }
                continue;
            }
            if self.stream_mesh_waiting_in(pos, &nbhd) {
                self.side.terrain.dirty_meshes.push(pos);
                continue;
            }
            if let Some(job) = self.build_mesh_job_from(pos, &nbhd) {
                let key = target.map_or(0, |t| t.section_priority_key(pos));
                let cancel = self.side.terrain.mesh_pool.submit(key, job);
                self.side.terrain.mesh_job_cancels.insert(pos, cancel);
                self.side.terrain.mesh_jobs_in_flight += 1;
                submitted += 1;
                if submitted >= target_jobs
                    || (submitted > 0 && submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET)
                {
                    for &rest in &candidates[i + 1..] {
                        self.side.terrain.dirty_meshes.push(rest);
                    }
                    break;
                }
            }
        }
    }

    #[cfg(test)]
    pub fn mesh_section_blocking_for_test(&mut self, pos: SectionPos) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.data
                    .ensure_column(ChunkPos::new(pos.cx + dx, pos.cz + dz));
            }
        }
        for _ in 0..256 {
            self.tick_mesh_budget(8);
            let ready = self.side.terrain.meshes.contains_key(&pos)
                && self.data.sections.get(&pos).is_none_or(|s| !s.dirty);
            if ready {
                return;
            }
        }
        panic!("mesh for {pos:?} did not complete");
    }
}
