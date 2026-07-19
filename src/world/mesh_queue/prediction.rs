use rustc_hash::FxHashSet;
use std::sync::Arc;

use crate::chunk::{self, ChunkPos, SectionPos};
use crate::world::store::{SkyCoverChange, World};

impl World {
    /// Present a reconciliation/rollback edit without blocking the client
    /// owner thread. The corrective light and meshes publish together after
    /// revision validation.
    ///
    /// `previous` carries each cell's block id before the predicted mutation;
    /// the world already contains the post-edit state when this is called.
    pub fn reconcile_predicted_edit(&mut self, previous: &[(crate::mathh::IVec3, u8)]) {
        if previous.is_empty() {
            return;
        }
        let Some(work) = self.prepare_prediction_terrain(previous) else {
            return;
        };
        let requeue = self.prediction_terrain.submit(work);
        self.requeue_prediction_meshes(&requeue);
    }

    /// Present an initial local break/place prediction. The complete affected
    /// light -> mesh bundle runs on the caller, so exact predicted presentation
    /// is installed before this method returns. Reconciliation uses
    /// [`Self::reconcile_predicted_edit`] and remains asynchronous.
    pub fn present_predicted_edit(&mut self, previous: &[(crate::mathh::IVec3, u8)]) {
        use crate::world::prediction_render::run_prediction_terrain_synchronously;

        if previous.is_empty() {
            return;
        }
        let Some(work) = self.prepare_prediction_terrain(previous) else {
            return;
        };
        let guarded: Vec<_> = work.guards.iter().map(|guard| guard.pos).collect();
        let requeue = self.prediction_terrain.cancel_overlapping(&guarded);
        self.requeue_prediction_meshes(&requeue);
        let pool = Arc::clone(self.prediction_terrain.pool());
        let result = run_prediction_terrain_synchronously(work, &pool)
            .expect("an uncancelled synchronous prediction bundle completes");
        let installed = self.install_prediction_terrain_result(result);
        debug_assert!(installed, "the owner cannot mutate a synchronous snapshot");
    }

    fn prepare_prediction_terrain(
        &mut self,
        previous: &[(crate::mathh::IVec3, u8)],
    ) -> Option<crate::world::prediction_render::PredictionTerrainWork> {
        use crate::world::light::{group_positions, snapshot_batch, LightBakeJob};
        use crate::world::prediction_render::{
            PredictionLightJob, PredictionLightUnit, PredictionMeshJob, PredictionTerrainWork,
            SectionGuard,
        };

        // Everything the edit can possibly require: light-influence reach plus
        // one pad cell, and the widened direct-sky cover band. The post-bake
        // diff prunes whatever the fresh cubes prove untouched; all of what
        // survives must publish atomically.
        let (candidates, always_mesh) = self.prediction_candidate_sections(previous);
        if candidates.is_empty() {
            return None;
        }
        let mut sampled = Vec::new();
        let mut sampled_seen = FxHashSet::default();
        for &mesh_pos in &candidates {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let pos =
                            SectionPos::new(mesh_pos.cx + dx, mesh_pos.cy + dy, mesh_pos.cz + dz);
                        if self.sections.contains_key(&pos) && sampled_seen.insert(pos) {
                            sampled.push(pos);
                        }
                    }
                }
            }
        }
        let light_positions: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|pos| {
                self.sections
                    .get(pos)
                    .is_some_and(|section| section.light_dirty && !section.all_opaque())
            })
            .collect();

        let guards: Vec<SectionGuard> = sampled
            .into_iter()
            .filter_map(|pos| {
                self.sections.get(&pos).map(|section| SectionGuard {
                    pos,
                    light_revision: section.light_revision,
                    mesh_revision: section.mesh_revision,
                })
            })
            .collect();
        // Mirror streaming: groups of 3+ share one 64³ batch flood; smaller
        // groups keep the per-section 48³ bake (below three the shared cube
        // costs more cells than separate floods).
        let mut lights = Vec::new();
        for (base, members) in group_positions(&light_positions) {
            if members.len() >= 3 {
                let Some(job) = snapshot_batch(base, &members, &self.sections, &self.columns)
                else {
                    return None;
                };
                let mut prev = Vec::with_capacity(members.len());
                for pos in job.member_positions() {
                    let section = self.sections.get(&pos).expect("batch members are loaded");
                    prev.push((section.skylight_arc(), section.blocklight_arc()));
                }
                lights.push(PredictionLightUnit::Batch { job, prev });
            } else {
                for pos in members {
                    let Some(job) =
                        LightBakeJob::snapshot(0, pos, &self.sections, &self.columns)
                    else {
                        return None;
                    };
                    let section = self.sections.get(&pos).expect("filtered on presence");
                    lights.push(PredictionLightUnit::Single(PredictionLightJob {
                        job,
                        prev_skylight: section.skylight_arc(),
                        prev_blocklight: section.blocklight_arc(),
                    }));
                }
            }
        }
        let mut meshes = Vec::with_capacity(candidates.len());
        for pos in candidates {
            let Some(section) = self.sections.get(&pos) else {
                continue;
            };
            if section.is_empty_air() {
                meshes.push(PredictionMeshJob::Remove {
                    pos,
                    revision: section.mesh_revision,
                });
            } else if let Some(job) = self.build_mesh_job(pos) {
                meshes.push(PredictionMeshJob::Build(job));
            }
        }
        if meshes.is_empty() {
            return None;
        }

        // A regular bake from an older prediction cannot contribute to this
        // snapshot and would only race it for worker time.
        for &pos in &light_positions {
            self.light_bakes.cancel(pos);
        }
        Some(PredictionTerrainWork {
            guards,
            lights,
            meshes,
            always_mesh,
        })
    }

    /// (candidate sections, geometry samplers) for a predicted edit. The
    /// candidates over-approximate what may need publishing — sections with a
    /// cell within one pad cell of the light-influence reach (L1 15) of an
    /// edited cell, plus the widened sky-cover band — and the bundle's
    /// post-bake diff prunes them down. The geometry samplers (pads containing
    /// an edited cell) rebuild unconditionally.
    fn prediction_candidate_sections(
        &self,
        previous: &[(crate::mathh::IVec3, u8)],
    ) -> (Vec<SectionPos>, Vec<SectionPos>) {
        const SAMPLER_REACH: i32 = chunk::SKY_FULL as i32 / 2 - 1 + 1;
        let mut candidates = Vec::new();
        let mut seen = FxHashSet::default();
        let mut always_mesh: Vec<SectionPos> = Vec::new();
        for &(cell, _) in previous {
            let Some((center, lx, ly, lz)) = World::split_world(cell.x, cell.y, cell.z) else {
                continue;
            };
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let gap = World::axis_gap(lx, dx)
                            + World::axis_gap(ly, dy)
                            + World::axis_gap(lz, dz);
                        if gap > SAMPLER_REACH {
                            continue;
                        }
                        let pos = SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz);
                        if !self.sections.contains_key(&pos) {
                            continue;
                        }
                        if seen.insert(pos) {
                            candidates.push(pos);
                        }
                        // The pad samples one cell across each bordering face.
                        let samples_cell = (dx == 0 || World::axis_gap(lx, dx) == 1)
                            && (dy == 0 || World::axis_gap(ly, dy) == 1)
                            && (dz == 0 || World::axis_gap(lz, dz) == 1);
                        if samples_cell && !always_mesh.contains(&pos) {
                            always_mesh.push(pos);
                        }
                    }
                }
            }
        }

        // Reconstruct the pre-edit sky cover for each changed world column.
        // This handles multi-cell edits (for example a door) as one before/after
        // comparison instead of mistaking another newly-written cell for old
        // cover while scanning downward.
        let mut changed_columns = Vec::new();
        for &(cell, _) in previous {
            let xz = (cell.x, cell.z);
            if !changed_columns.contains(&xz) {
                changed_columns.push(xz);
            }
        }
        for (wx, wz) in changed_columns {
            let cpos = ChunkPos::new(
                wx.div_euclid(chunk::SECTION_SIZE as i32),
                wz.div_euclid(chunk::SECTION_SIZE as i32),
            );
            let Some(column) = self.columns.get(&cpos) else {
                continue;
            };
            let new_cover = column.sky_cover_y(chunk::lx(wx), chunk::lz(wz));
            // Above both the current cover and the highest edited cell the
            // pre-edit column transmits everywhere, so the old cover can only
            // sit at or below that start line — no full-height scan.
            let start = previous
                .iter()
                .filter(|(cell, _)| cell.x == wx && cell.z == wz)
                .map(|(cell, _)| cell.y)
                .max()
                .unwrap_or(chunk::WORLD_MIN_Y)
                .max(new_cover)
                .min(chunk::WORLD_MAX_Y - 1);
            let old_cover = (chunk::WORLD_MIN_Y..=start)
                .rev()
                .find(|&wy| {
                    let old_id = previous
                        .iter()
                        .find(|(cell, _)| cell.x == wx && cell.y == wy && cell.z == wz)
                        .map_or_else(|| self.chunk_block(wx, wy, wz), |(_, id)| *id);
                    !crate::block::Block::from_id(old_id).transmits_direct_skylight()
                })
                .unwrap_or(crate::column::NO_SURFACE);
            let Some(change) = SkyCoverChange::between(old_cover, new_cover) else {
                continue;
            };
            for cz in cpos.cz - 1..=cpos.cz + 1 {
                for cx in cpos.cx - 1..=cpos.cx + 1 {
                    let cp = ChunkPos::new(cx, cz);
                    let bits = self.section_column_cys.get(&cp).copied().unwrap_or(0);
                    let mut b = bits;
                    while b != 0 {
                        let cy = chunk::SECTION_MIN_CY + b.trailing_zeros() as i32;
                        b &= b - 1;
                        let pos = SectionPos::new(cx, cy, cz);
                        if change.segment_gap(pos, wx, wz) <= SAMPLER_REACH && seen.insert(pos)
                        {
                            candidates.push(pos);
                        }
                    }
                }
            }
        }
        (candidates, always_mesh)
    }

    /// Install completed local-prediction terrain bundles on the replica
    /// owner thread. Freshness is all-or-nothing: any sampled section change,
    /// authoritative light landing, unload, or newer prediction rejects the
    /// entire bundle and hands its mesh targets back to the ordinary pipeline.
    pub(super) fn drain_prediction_terrain(&mut self) {
        while let Some(completion) = self.prediction_terrain.try_recv() {
            let installed = completion
                .result
                .is_some_and(|result| self.install_prediction_terrain_result(result));
            if !installed {
                self.requeue_prediction_meshes(&completion.mesh_positions);
            }
        }
    }

    fn install_prediction_terrain_result(
        &mut self,
        result: crate::world::prediction_render::PredictionTerrainResult,
    ) -> bool {
        use crate::world::prediction_render::{PredictionMeshResult, PredictionTerrainResult};

        let guards_fresh = result.guards.iter().all(|guard| {
            self.sections.get(&guard.pos).is_some_and(|section| {
                section.light_revision == guard.light_revision
                    && section.mesh_revision == guard.mesh_revision
            })
        });
        let lights_fresh = result.lights.iter().all(|light| {
            self.sections.get(&light.result.pos).is_some_and(|section| {
                section.light_dirty && section.light_revision == light.result.revision
            })
        });
        let meshes_fresh = result.meshes.iter().all(|mesh| {
            self.sections
                .get(&mesh.pos())
                .is_some_and(|section| section.mesh_revision == mesh.revision())
        });
        if !(guards_fresh && lights_fresh && meshes_fresh) {
            return false;
        }

        let PredictionTerrainResult {
            guards: _,
            lights,
            meshes,
        } = result;
        let mut changed_light = false;
        let mut rim_requeues: Vec<(SectionPos, u32)> = Vec::new();
        for light in lights {
            let pos = light.result.pos;
            self.light_bakes.cancel(pos);
            if let Some(section) = self.section_mut(pos) {
                if light.mask == 0 {
                    // Byte-identical rebake: the cached cubes and every mesh
                    // built from them remain exact — settle the flag only.
                    section.mark_light_clean();
                    continue;
                }
                changed_light = true;
                section.set_skylight(light.result.skylight);
                section.set_blocklight(light.result.blocklight);
                section.dirty = true;
                // Any ordinary mesh snapshot from before this bundle's
                // light stage is stale. The bundled mesh below is known to
                // contain these exact cubes and installs past this bump.
                section.mesh_revision = section.mesh_revision.wrapping_add(1);
                if !light.first_bake {
                    rim_requeues.push((pos, light.mask));
                }
            }
        }
        if changed_light {
            self.bump_lighting_revision();
        }

        let mut installed = FxHashSet::default();
        for mesh in meshes {
            let pos = mesh.pos();
            if let Some(job) = self.mesh_job_cancels.get(&pos) {
                job.cancel();
            }
            match mesh {
                PredictionMeshResult::Built { mut mesh, .. } => {
                    mesh.mesh_dirty = true;
                    self.install_mesh(pos, mesh);
                }
                PredictionMeshResult::Remove { .. } => {
                    if self.remove_mesh(pos) {
                        self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
                    }
                }
            }
            installed.insert(pos);
            self.dirty_meshes.remove(pos);
            self.light_blocked_meshes.remove(&pos);
            self.hidden_parked.remove(&pos);
            self.sealed_parked.remove(&pos);
            self.light_deferred.remove(&pos);
            self.deferred_rechecks.remove(&pos);
            if let Some(section) = self.section_mut(pos) {
                section.dirty = false;
            }
        }
        // A changed border region whose sampling neighbour got no bundle mesh
        // (it sat outside the candidate reach) still invalidates that
        // neighbour's installed mesh — hand it to the ordinary pipeline.
        for (pos, mask) in rim_requeues {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        if (dx, dy, dz) == (0, 0, 0)
                            || mask & crate::world::light::region_bit(dx, dy, dz) == 0
                        {
                            continue;
                        }
                        let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                        if installed.contains(&p)
                            || self.dirty_meshes.contains(p)
                            || self.light_blocked_meshes.contains(&p)
                            || !self.sections.contains_key(&p)
                        {
                            continue;
                        }
                        self.queue_dirty_mesh(p);
                    }
                }
            }
        }
        self.flush_light_blocked_meshes();
        true
    }

    fn requeue_prediction_meshes(&mut self, positions: &[SectionPos]) {
        for &pos in positions {
            self.light_blocked_meshes.remove(&pos);
            if self.sections.contains_key(&pos) {
                self.dirty_meshes.push(pos);
            }
        }
    }
}
