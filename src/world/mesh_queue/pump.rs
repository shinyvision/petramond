#[cfg(test)]
use crate::chunk::{ChunkPos, SectionPos};
use crate::world::store::World;

use super::{
    max_mesh_jobs_in_flight, CANDIDATE_SCAN_PER_MESH_JOB, MESH_SUBMIT_TIME_BUDGET,
    MIN_MESH_JOBS_PER_PUMP,
};

impl World {
    /// Drain finished meshes and submit newly-dirty sections to the off-thread mesh
    /// pool, capped per frame. The render thread never builds a mesh here — it only
    /// snapshots a section + its neighbourhood (cheap) and drains results — so a heavy
    /// streaming frame can't stall it.
    pub fn tick_mesh_budget(&mut self, max_per_frame: usize) {
        self.mesh_pump_frame += 1;
        self.drain_prediction_terrain();
        self.pump_light_bakes();
        self.drain_finished_meshes();
        self.release_settled_column_meshes();
        if max_per_frame == 0 {
            return;
        }
        // The submit budget anchors AFTER the drains: each drain bounds its own
        // time, and letting their cost count against submission starved meshing
        // to a trickle exactly when ingest churn was heaviest (RD32 sprint
        // flight: 14k dirty backlog, sub-500 installed meshes, idle workers).
        let submit_start = std::time::Instant::now();

        // Never let the pool's FIFO channel outgrow the cap: leave the rest of the backlog in
        // the nearest-first `dirty_meshes` so a just-edited section isn't stuck behind it.
        let in_flight_room = max_mesh_jobs_in_flight().saturating_sub(self.mesh_jobs_in_flight);
        if in_flight_room == 0 {
            return;
        }
        let target_jobs = max_per_frame
            .max(MIN_MESH_JOBS_PER_PUMP)
            .min(in_flight_room);
        let candidate_cap = target_jobs.saturating_mul(CANDIDATE_SCAN_PER_MESH_JOB);
        // Ingest can raise this every frame while thousands of sections arrive.
        // One refresh per eight frames keeps the O(deep) BFS bounded; parked
        // sections re-enter automatically on the next refresh.
        if self.vis_dirty && self.mesh_pump_frame.is_multiple_of(8) {
            self.refresh_deep_visibility();
        }
        if submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
            return;
        }
        let target = self.last_load_target;
        let candidates = self.dirty_meshes.pop_nearest_batch(candidate_cap, target);
        let mut submitted = 0usize;
        for (i, &pos) in candidates.iter().enumerate() {
            if submitted > 0 && submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                for &rest in &candidates[i..] {
                    self.dirty_meshes.push(rest);
                }
                break;
            }
            if !self.sections.contains_key(&pos) {
                continue;
            }
            // A predicted light -> mesh bundle owns this presentation until
            // its revision-fresh result lands. In particular, do not clear an
            // all-air section's old mesh between the two atomic stages.
            if self.prediction_terrain.owns_mesh(pos) {
                self.light_blocked_meshes.insert(pos);
                continue;
            }
            // Hidden deep section: nothing can see it — park it out of the hot
            // queue (its light parks with it, since light is mesh-demanded). The
            // visibility refresh re-queues it the moment a sightline can reach it.
            // Repack-forced sections are exempt: their (released) geometry is still
            // part of the packed column buffer, so the repack needs a fresh mesh
            // even if nothing can currently see the section.
            if self.section_hidden(pos) && !self.repack_forced.contains(&pos) {
                self.hidden_parked.insert(pos);
                continue;
            }
            // All-air sections emit nothing: settle them (dropping any ghost mesh)
            // instead of meshing.
            if self.clear_mesh_if_section_produces_no_mesh(pos) {
                if submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
                continue;
            }
            if self.section_sealed_by_loaded_neighbors(pos) && !self.repack_forced.contains(&pos) {
                self.sealed_parked.insert(pos);
                self.dirty_meshes.remove(pos);
                self.light_blocked_meshes.remove(&pos);
                if let Some(s) = self.section_mut(pos) {
                    s.dirty = false;
                    // Invalidate a snapshot taken before the final sealing
                    // neighbour landed. Keep any installed mesh as the fail-safe.
                    s.mesh_revision = s.mesh_revision.wrapping_add(1);
                }
                continue;
            }
            // Don't snapshot from stale light: a section whose 3×3×3 light isn't baked
            // yet parks outside the hot dirty queue, so the snapshot always carries final light.
            if self.request_light_dependencies(pos) {
                self.light_blocked_meshes.insert(pos);
                if submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
                continue;
            }
            if let Some(job) = self.build_mesh_job(pos) {
                let key = target.map_or(0, |t| t.section_priority_key(pos));
                let cancel = self.mesh_pool.submit(key, job);
                self.mesh_job_cancels.insert(pos, cancel);
                self.mesh_jobs_in_flight += 1;
                submitted += 1;
                if submitted >= target_jobs
                    || (submitted > 0 && submit_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET)
                {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
            }
        }
    }

    /// Synchronously mesh `pos` for a test: meshing is async now, so pump the budget +
    /// drain until the section's mesh lands (or time out).
    #[cfg(test)]
    pub(crate) fn mesh_section_blocking_for_test(&mut self, pos: SectionPos) {
        use std::time::{Duration, Instant};
        for dz in -1..=1 {
            for dx in -1..=1 {
                self.ensure_column(ChunkPos::new(pos.cx + dx, pos.cz + dz));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            self.tick_mesh_budget(8);
            // Up to date once a mesh exists AND the section isn't queued/in-flight for a
            // fresher one (a re-dirty sets `dirty`, the drained result clears it).
            let ready =
                self.meshes.contains_key(&pos) && self.sections.get(&pos).is_none_or(|s| !s.dirty);
            if ready {
                return;
            }
            if Instant::now() >= deadline {
                panic!("mesh for {pos:?} did not complete");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
