use crate::world::{ReplicaWorld, ServerWorld, World, WorldSide};
use petramond_world::chunk::{self, SectionPos};

use super::{RESULT_DRAIN_MIN, RESULT_DRAIN_TIME_BUDGET};

/// A bake that landed and changed a section's cached light.
pub(in crate::world) struct LandedLight {
    pub(in crate::world) pos: SectionPos,
    /// The section had no baked light before (its sampling neighbours were
    /// parked on it and rebuild anyway).
    pub(in crate::world) first_bake: bool,
    /// Which border regions' cells changed (`light::region_bit`).
    pub(in crate::world) mask: u32,
}

impl ServerWorld {
    /// Drain and apply finished light bakes with no mesh machinery attached —
    /// the server's light pump. A landed bake is new shippable content:
    /// `LightData` for recipients that already hold the section, and (via the
    /// terrain revision) a replan for those still waiting on the light-final
    /// ship gate.
    ///
    /// Queued incremental relights drain first: they install exact cubes on
    /// the spot, and whatever they decline lands in `relight_demand` in time
    /// for this same pump to request its full rebakes.
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
                        .data.sections
                        .get(pos)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque());
                    // Deferred first-timers bake once their gen neighbourhood
                    // settles (streamer-owned), and a prediction bundle bakes its
                    // own snapshot — requesting here would double-bake either.
                    bakeable
                        && !self.data.light_deferred.contains(pos)
                        && !self
                            .side
                            .replica()
                            .is_some_and(|r| r.terrain.prediction_terrain.owns_light(*pos))
                })
                .collect();
            // Streaming seam rebakes arrive in adjacent bursts; groups of 3+
            // share one 64³ batch flood (see `light::batch`), smaller groups
            // keep the per-section 48³ bake.
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
            let Some(res) = self.light_bakes.try_recv() else {
                break;
            };
            drained += 1;
            let fresh = self
                .data.sections
                .get(&res.pos)
                .is_some_and(|s| s.light_dirty && s.light_revision == res.revision);
            if !fresh {
                // A stale rejection is the moment the section has NO bake in
                // flight anymore (`try_recv` cleared the pending slot) while
                // every request made during the flight was dedup-dropped. If it
                // is still dirty, re-request here or it wedges light-dirty and
                // every mesh whose 3×3×3 reads it parks in
                // `light_blocked_meshes` until an unrelated edit.
                let rebake = self
                    .data.sections
                    .get(&res.pos)
                    .is_some_and(|s| s.light_dirty && !s.all_opaque())
                    && !self.data.light_deferred.contains(&res.pos);
                if rebake {
                    let key = self
                        .data.last_load_target
                        .map_or(0, |t| t.section_priority_key(res.pos));
                    self.light_bakes
                        .request(key, res.pos, &self.data.sections, &self.data.columns);
                }
                continue;
            }
            let Some(s) = self.data.section_mut(res.pos) else {
                continue;
            };
            // Region-diff the landing cubes against the cached ones so a
            // rebake that changed nothing (a light-neutral edit in range, a
            // re-request race) publishes nothing, and a real change requeues
            // exactly the meshes that sampled the changed cells. A first bake
            // reads as changed-everywhere; its sampling neighbours were parked
            // on this section's `light_dirty`, so they rebuild anyway.
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
                // Byte-identical rebake: the cached cubes and every mesh built
                // from them remain exact — just settle the dirty flag.
                s.mark_light_clean();
                if persisting {
                    // The pending edit-staleness resolved: the cells' light is
                    // proven unchanged, so any persisted cubes remain exact.
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

    /// Install changed light cubes on `pos` — a landed bake, or an incremental
    /// relight — with the persistence bookkeeping. Publishing the change is
    /// the side's job: a replica requeues meshes, the server ships it.
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
        // The cached light changed, so any in-flight mesh built from the old
        // light is now stale: bump so its result is discarded and re-queue.
        s.mesh_revision = s.mesh_revision.wrapping_add(1);
        self.data.bump_lighting_revision();
        if persisting {
            // An already-persisted record must rewrite with the new cubes
            // (see `relit_since_persist`); unknown-to-disk sections are
            // filtered at the persist gate. The fresh cubes also resolve any
            // pending edit-staleness — they supersede it.
            self.data.relit_since_persist.insert(pos);
            self.data.light_edited_since_persist.remove(&pos);
        }
    }
}

impl ReplicaWorld {
    /// The replica's light pump (run from `tick_mesh_budget`): a landed
    /// change requeues the section's own mesh and every neighbour mesh that
    /// sampled the changed cells, then parked meshes whose light is ready
    /// re-enter the queue.
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
    /// A landed rebake changed cells in some of `pos`'s border regions: any
    /// neighbour whose installed or in-flight mesh sampled those cells through
    /// its one-cell pad must rebuild. Already queued/parked neighbours are left
    /// alone — they will build against the fresh cube anyway.
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

    /// Queue every dirty light cube a section mesh would read from its 3×3×3
    /// sampling neighbourhood. Returns true when the mesh must wait for async light.
    ///
    /// Fully-opaque neighbours are skipped: their cells are solid, so a meshed neighbour's
    /// faces are culled against them and never sample their light — baking it would be
    /// wasted, and waiting on it would stall the mesh. (Carving air in clears `all_opaque`,
    /// so it rejoins the light path then.)
    pub(super) fn request_light_dependencies(&mut self, pos: SectionPos) -> bool {
        let mut waiting = false;
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self
                        .data.sections
                        .get(&p)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque())
                        && !self.section_sealed_by_loaded_neighbors(p)
                    {
                        // A deferred neighbour's first bake fires when its own
                        // neighbourhood settles (`flush_settled_deferred`); requesting
                        // it here would bake a half-landed neighbourhood and be
                        // immediately redone. Still wait on it.
                        if !self.data.light_deferred.contains(&p)
                            && !self.side.terrain.prediction_terrain.owns_light(p)
                        {
                            let key = self
                                .data.last_load_target
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
                        .data.sections
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
            .side.terrain
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
