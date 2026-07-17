use crate::chunk::{self, SectionPos};
use crate::world::store::World;

use super::{RESULT_DRAIN_MIN, RESULT_DRAIN_TIME_BUDGET};

impl World {
    /// Drain and apply finished light bakes — the light half of the pump,
    /// public so a headless server loop can keep light current with no mesh
    /// machinery attached. `tick_mesh_budget` calls this internally, so the
    /// combined/client worlds behave exactly as before.
    ///
    /// This is ALSO where marked rebakes are REQUESTED: edits mark light
    /// dirty into `relight_demand` (`mark_light_dirty_pos`), so invalidated
    /// light rebakes even when no queued mesh demands it — a distant
    /// sky-cover segment whose meshes only requeue if the landed cubes prove
    /// changed, or a headless server with no mesh pump at all. First-time
    /// bakes still come from the streamer's `flush_settled_deferred`.
    pub fn pump_light_bakes(&mut self) {
        if !self.relight_demand.is_empty() {
            let target = self.last_load_target;
            let bakes: Vec<SectionPos> = std::mem::take(&mut self.relight_demand)
                .into_iter()
                .filter(|pos| {
                    let bakeable = self
                        .sections
                        .get(pos)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque());
                    // Deferred first-timers bake once their gen neighbourhood
                    // settles (streamer-owned), and a prediction bundle bakes its
                    // own snapshot — requesting here would double-bake either.
                    bakeable
                        && !self.light_deferred.contains(pos)
                        && !self.prediction_terrain.owns_light(*pos)
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
                    self.light_bakes
                        .request_batch(key, base, &members, &self.sections, &self.columns);
                } else {
                    for pos in members {
                        let key = target.map_or(0, |t| t.section_priority_key(pos));
                        self.light_bakes
                            .request(key, pos, &self.sections, &self.columns);
                    }
                }
            }
        }
        let start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < RESULT_DRAIN_MIN || start.elapsed() < RESULT_DRAIN_TIME_BUDGET {
            let Some(res) = self.light_bakes.try_recv() else {
                break;
            };
            drained += 1;
            let fresh = self
                .sections
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
                    .sections
                    .get(&res.pos)
                    .is_some_and(|s| s.light_dirty && !s.all_opaque())
                    && !self.light_deferred.contains(&res.pos);
                if rebake {
                    let key = self
                        .last_load_target
                        .map_or(0, |t| t.section_priority_key(res.pos));
                    self.light_bakes
                        .request(key, res.pos, &self.sections, &self.columns);
                }
                continue;
            }
            let Some(s) = self.section_mut(res.pos) else {
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
                    0,
                )
            };
            if mask == 0 {
                // Byte-identical rebake: the cached cubes and every mesh built
                // from them remain exact — just settle the dirty flag.
                s.mark_light_clean();
                if self.save.is_some() {
                    // The pending edit-staleness resolved: the cells' light is
                    // proven unchanged, so any persisted cubes remain exact.
                    self.light_edited_since_persist.remove(&res.pos);
                }
                continue;
            }
            s.set_skylight(res.skylight);
            s.set_blocklight(res.blocklight);
            s.dirty = true;
            // The cached light changed, so any in-flight mesh built from the old
            // light is now stale: bump so its result is discarded and re-queue.
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
            self.bump_lighting_revision();
            if self.save.is_some() {
                // An already-persisted record must rewrite with the new cubes
                // (see `relit_since_persist`); unknown-to-disk sections are
                // filtered at the persist gate. The landed bake also resolves
                // any pending edit-staleness — the fresh cubes supersede it.
                self.relit_since_persist.insert(res.pos);
                self.light_edited_since_persist.remove(&res.pos);
            }
            if self.role == crate::world::store::WorldRole::ServerHeadless {
                // A landed bake is new shippable content: LightData for
                // recipients that already hold the section, and (via the
                // revision) a replan for those still waiting on the light-final
                // ship gate. No meshes to relight headless (`queue_dirty_mesh`).
                self.light_ship_log.insert(res.pos);
                self.bump_terrain_revision();
            } else {
                self.dirty_meshes.push(res.pos);
                if !first_bake {
                    self.requeue_meshes_sampling_changed_regions(res.pos, mask);
                }
            }
        }
        self.flush_light_blocked_meshes();
    }

    /// A landed rebake changed cells in some of `pos`'s border regions: any
    /// neighbour whose installed or in-flight mesh sampled those cells through
    /// its one-cell pad must rebuild. Already queued/parked neighbours are left
    /// alone — they will build against the fresh cube anyway.
    pub(in crate::world) fn requeue_meshes_sampling_changed_regions(&mut self, pos: SectionPos, mask: u32) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if (dx, dy, dz) == (0, 0, 0)
                        || mask & crate::world::light::region_bit(dx, dy, dz) == 0
                    {
                        continue;
                    }
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self.dirty_meshes.contains(p)
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
                        .sections
                        .get(&p)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque())
                        && !self.section_sealed_by_loaded_neighbors(p)
                    {
                        // A deferred neighbour's first bake fires when its own
                        // neighbourhood settles (`flush_settled_deferred`); requesting
                        // it here would bake a half-landed neighbourhood and be
                        // immediately redone. Still wait on it.
                        if !self.light_deferred.contains(&p)
                            && !self.prediction_terrain.owns_light(p)
                        {
                            let key = self
                                .last_load_target
                                .map_or(0, |t| t.section_priority_key(p));
                            self.light_bakes
                                .request(key, p, &self.sections, &self.columns);
                        }
                        waiting = true;
                    }
                }
            }
        }
        waiting
    }

    fn mesh_light_dependencies_pending(&self, pos: SectionPos) -> bool {
        if self.prediction_terrain.owns_mesh(pos) {
            return true;
        }
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self
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
        if self.light_blocked_meshes.is_empty() {
            return;
        }
        let ready: Vec<SectionPos> = self
            .light_blocked_meshes
            .iter()
            .copied()
            .filter(|&pos| {
                !self.sections.contains_key(&pos) || !self.mesh_light_dependencies_pending(pos)
            })
            .collect();
        for pos in ready {
            self.light_blocked_meshes.remove(&pos);
            if self.sections.contains_key(&pos) {
                self.dirty_meshes.push(pos);
            }
        }
    }
}
