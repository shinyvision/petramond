use crate::world::{ReplicaWorld, ServerWorld, World, WorldSide};
use petramond_world::chunk::{self, SectionPos};

use super::{RESULT_DRAIN_MIN, RESULT_DRAIN_TIME_BUDGET};

pub(in crate::world) struct LandedLight {
    pub(in crate::world) pos: SectionPos,
    pub(in crate::world) first_bake: bool,
    pub(in crate::world) mask: u32,
}

impl ServerWorld {
    pub fn pump_light_bakes(&mut self) {
        self.apply_light_edits();
        for landed in self.drain_light_bakes() {
            self.side.replication.light_ship_log.insert(landed.pos);
            self.side.replication.bump_terrain_revision();
        }
    }
}

impl<S: WorldSide> World<S> {
    /// The light half of either side's pump: request marked rebakes and apply
    /// the landed ones, returning the sections whose cached light changed.
    ///
    /// This is ALSO where marked rebakes are REQUESTED: edits mark light
    /// dirty into `relight_demand` (`mark_light_dirty_pos`), so invalidated
    /// light rebakes even when no queued mesh demands it — a distant
    /// sky-cover segment whose meshes only requeue if the landed cubes prove
    /// changed, or the server with no mesh pump at all. First-time bakes
    /// still come from the streamer's `flush_settled_deferred`.
    pub(in crate::world) fn drain_light_bakes(&mut self) -> Vec<LandedLight> {
        let mut landed = Vec::new();
        if !self.data.relight_demand.is_empty() {
            let target = self.data.last_load_target;
            let bakes: Vec<SectionPos> = std::mem::take(&mut self.data.relight_demand)
                .into_iter()
                .filter(|pos| {
                    let bakeable = self
                        .data
                        .sections
                        .get(pos)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque());
                    bakeable
                        && !self.data.light_deferred.contains(pos)
                        && !self
                            .side
                            .replica()
                            .is_some_and(|r| r.terrain.prediction_terrain.owns_light(*pos))
                })
                .collect();
            for (base, members) in crate::world::light::group_positions(&bakes) {
                if members.len() >= 3 {
                    let key = members
                        .iter()
                        .map(|&sp| target.map_or(0, |t| t.section_priority_key(sp)))
                        .min()
                        .unwrap_or(0);
                    self.light_bakes.request_batch(
                        key,
                        base,
                        &members,
                        &self.data.sections,
                        &self.data.columns,
                    );
                } else {
                    for pos in members {
                        let key = target.map_or(0, |t| t.section_priority_key(pos));
                        self.light_bakes
                            .request(key, pos, &self.data.sections, &self.data.columns);
                    }
                }
            }
        }
        let persisting = self.persisting();
        let start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < RESULT_DRAIN_MIN || start.elapsed() < RESULT_DRAIN_TIME_BUDGET {
            let Some(event) = self.light_bakes.try_recv() else {
                break;
            };
            drained += 1;
            let res = match event {
                crate::world::light::LightBakeEvent::Baked(res) => res,
                crate::world::light::LightBakeEvent::Failed { pos, revision } => {
                    landed.extend(self.settle_failed_light_bake(pos, revision));
                    continue;
                }
            };
            let fresh = self
                .data
                .sections
                .get(&res.pos)
                .is_some_and(|s| s.light_dirty && s.light_revision == res.revision);
            if !fresh {
                self.rerequest_stale_bake(res.pos);
                continue;
            }
            let Some(s) = self.data.section_mut(res.pos) else {
                continue;
            };
            let first_bake = !s.has_baked_light();
            let mask = if first_bake {
                crate::world::light::REGION_ALL
            } else {
                crate::world::light::cube_region_changes(
                    s.skylight_arc().as_deref(),
                    &res.skylight,
                    chunk::SKY_FULL,
                ) | crate::world::light::cube_region_changes(
                    s.blocklight_arc().as_deref(),
                    &res.blocklight,
                    petramond_world::light::LightRgb::ZERO,
                )
            };
            if mask == 0 {
                s.mark_light_clean();
                if persisting {
                    self.data.light_edited_since_persist.remove(&res.pos);
                }
                continue;
            }
            self.install_light_cubes(res.pos, res.skylight, res.blocklight);
            landed.push(LandedLight {
                pos: res.pos,
                first_bake,
                mask,
            });
        }
        landed
    }

    /// A result for `pos` arrived stale. That is the moment the section has NO
    /// bake in flight anymore (`try_recv` cleared the pending slot) while
    /// every request made during the flight was dedup-dropped. If it is still
    /// dirty, re-request here or it wedges light-dirty and every mesh whose
    /// 3×3×3 reads it parks in `light_blocked_meshes` until an unrelated edit.
    fn rerequest_stale_bake(&mut self, pos: SectionPos) {
        let rebake = self
            .data
            .sections
            .get(&pos)
            .is_some_and(|s| s.light_dirty && !s.all_opaque())
            && !self.data.light_deferred.contains(&pos);
        if rebake {
            let key = self
                .data
                .last_load_target
                .map_or(0, |t| t.section_priority_key(pos));
            self.light_bakes
                .request(key, pos, &self.data.sections, &self.data.columns);
        }
    }

    /// The bake of `pos` panicked (SEC-03). A failure against a revision the
    /// section has since moved past is just stale — re-request like a stale
    /// result. A CURRENT failure settles the section's light instead of
    /// leaving it dirty: dirty light parks every mesh sampling it, holds the
    /// section back from clients (the light-final ship gate), and a
    /// re-request would re-run a deterministic panic every pump. The section
    /// keeps its previous cubes; an unbaked one gets the uncomputed defaults
    /// (open sky, no block light) made explicit. Its next light-dirty mark (an
    /// edit, a neighbour landing) bakes it again.
    fn settle_failed_light_bake(&mut self, pos: SectionPos, revision: u64) -> Option<LandedLight> {
        let s = self.data.sections.get(&pos)?;
        if !(s.light_dirty && s.light_revision == revision) {
            self.rerequest_stale_bake(pos);
            return None;
        }
        let first_bake = !s.has_baked_light();
        let skylight = s
            .skylight_arc()
            .unwrap_or_else(|| std::sync::Arc::from(vec![chunk::SKY_FULL; chunk::SECTION_VOLUME]));
        let blocklight = s.blocklight_arc().unwrap_or_else(|| {
            std::sync::Arc::from(vec![
                petramond_world::light::LightRgb::ZERO;
                chunk::SECTION_VOLUME
            ])
        });
        self.install_light_cubes(pos, skylight, blocklight);
        Some(LandedLight {
            pos,
            first_bake,
            mask: crate::world::light::REGION_ALL,
        })
    }

    pub(in crate::world) fn install_light_cubes(
        &mut self,
        pos: SectionPos,
        skylight: std::sync::Arc<[u8]>,
        blocklight: std::sync::Arc<[petramond_world::light::LightRgb]>,
    ) {
        let persisting = self.persisting();
        let Some(s) = self.data.section_mut(pos) else {
            return;
        };
        s.set_skylight(skylight);
        s.set_blocklight(blocklight);
        s.dirty = true;
        s.mesh_revision = s.mesh_revision.wrapping_add(1);
        self.data.bump_lighting_revision();
        self.note_send_event(pos);
        if persisting {
            self.data.relit_since_persist.insert(pos);
            self.data.light_edited_since_persist.remove(&pos);
        }
    }
}

impl ReplicaWorld {
    pub fn pump_light_bakes(&mut self) {
        for landed in self.drain_light_bakes() {
            self.side.terrain.dirty_meshes.push(landed.pos);
            if !landed.first_bake {
                self.requeue_meshes_sampling_changed_regions(landed.pos, landed.mask);
            }
        }
        self.flush_light_blocked_meshes();
    }
}

impl ReplicaWorld {
    pub(in crate::world) fn requeue_meshes_sampling_changed_regions(
        &mut self,
        pos: SectionPos,
        mask: u32,
    ) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if (dx, dy, dz) == (0, 0, 0)
                        || mask & crate::world::light::region_bit(dx, dy, dz) == 0
                    {
                        continue;
                    }
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self.side.terrain.dirty_meshes.contains(p)
                        || self.side.terrain.light_blocked_meshes.contains(&p)
                        || !self.data.sections.contains_key(&p)
                    {
                        continue;
                    }
                    self.queue_dirty_mesh(p);
                }
            }
        }
    }

    pub(super) fn request_light_dependencies(
        &mut self,
        pos: SectionPos,
        nbhd: &super::mesh_jobs::MeshNbhd,
    ) -> bool {
        let mut waiting = false;
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if nbhd[crate::world::mesh_pool::nbhd_idx27(dx, dy, dz)]
                        .as_ref()
                        .is_some_and(|s| s.light_dirty && !s.all_opaque())
                        && !self.section_sealed_by_loaded_neighbors(p)
                    {
                        if !self.data.light_deferred.contains(&p)
                            && !self.side.terrain.prediction_terrain.owns_light(p)
                        {
                            let key = self
                                .data
                                .last_load_target
                                .map_or(0, |t| t.section_priority_key(p));
                            self.light_bakes.request(
                                key,
                                p,
                                &self.data.sections,
                                &self.data.columns,
                            );
                        }
                        waiting = true;
                    }
                }
            }
        }
        waiting
    }

    fn mesh_light_dependencies_pending(&self, pos: SectionPos) -> bool {
        if self.side.terrain.prediction_terrain.owns_mesh(pos) {
            return true;
        }
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self
                        .data
                        .sections
                        .get(&p)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque())
                        && !self.section_sealed_by_loaded_neighbors(p)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(super) fn flush_light_blocked_meshes(&mut self) {
        if self.side.terrain.light_blocked_meshes.is_empty() {
            return;
        }
        let ready: Vec<SectionPos> = self
            .side
            .terrain
            .light_blocked_meshes
            .iter()
            .copied()
            .filter(|&pos| {
                !self.data.sections.contains_key(&pos) || !self.mesh_light_dependencies_pending(pos)
            })
            .collect();
        for pos in ready {
            self.side.terrain.light_blocked_meshes.remove(&pos);
            if self.data.sections.contains_key(&pos) {
                self.side.terrain.dirty_meshes.push(pos);
            }
        }
    }
}
