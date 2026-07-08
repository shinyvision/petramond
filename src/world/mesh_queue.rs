use rustc_hash::FxHashSet;

use crate::chunk::{self, ChunkPos, SectionPos};

use super::store::{LoadTarget, World};

/// Minimum useful mesh submissions per pump. With the game-side budget intentionally set
/// to 1, a literal one-section budget makes the cubic streamer visibly crawl; this keeps
/// the tiny budget useful without multiplying larger diagnostic/tooling budgets.
const MIN_MESH_JOBS_PER_PUMP: usize = 16;
/// Scan past sections that are stale, no-mesh, or waiting on light so the budget still
/// launches useful work whenever any nearby section is ready. During streaming most
/// popped candidates PARK (light in flight / hidden deep) rather than submit, so the
/// scan must run well ahead of the submit count or parking throttles discovery to a
/// frame-quantized trickle. The submit time budget bounds the scan's real cost.
const CANDIDATE_SCAN_PER_MESH_JOB: usize = 4;
/// Bound result drains by TIME, not count: installs are cheap (Arc swaps + map
/// inserts), so a fixed small count needlessly frame-quantized streaming bursts
/// (24/frame = seconds of trickle for a flight burst the pool finished long ago).
/// The floor guarantees progress regardless of clock behaviour.
const RESULT_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(700);
const RESULT_DRAIN_MIN: usize = 24;
/// Cap on mesh jobs in flight in the shared pool. The pool queue is priority-ordered
/// (nearest first), so a fresh edit no longer queues behind the streaming backlog the
/// way it did with the old FIFO channel — this cap only bounds snapshot memory held
/// by queued jobs. The backlog beyond it stays in `dirty_meshes`, re-sorted
/// NEAREST-FIRST every frame.
const MAX_MESH_JOBS_IN_FLIGHT: usize = 32;
/// Soft main-thread budget for mesh-job snapshot submission. One useful submission is
/// always allowed; after that, the pump yields to rendering once it burns this much CPU.
const MESH_SUBMIT_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(2_000);
/// Mesh-pump frames a column must stay upload-quiet before its CPU mesh buffers are
/// released (~10 s at 60 fps). Releasing too early amplifies streaming work: any
/// repack of the column then has to remesh the released sections first.
pub(super) const MESH_RELEASE_DELAY_FRAMES: u64 = 600;
/// How often the release sweep scans `mesh_release_after` (it iterates the whole map,
/// so keep it off the every-frame path).
const MESH_RELEASE_SWEEP_INTERVAL: u64 = 64;

/// Set of sections awaiting a remesh. With `World`'s section map private, every
/// path that dirties a section pushes here and `remove_section` pulls it back out —
/// so the set alone says what needs meshing. Drained NEAREST-FIRST to the load
/// centre so the terrain around the player meshes before the edges.
#[derive(Default)]
pub(super) struct DirtyMeshQueue {
    pending: FxHashSet<SectionPos>,
    /// Reused across frames so `pop_nearest_batch` doesn't allocate a fresh `Vec` the
    /// size of the whole backlog every call.
    scratch: Vec<SectionPos>,
}

impl DirtyMeshQueue {
    pub fn push(&mut self, pos: SectionPos) {
        self.pending.insert(pos);
    }

    pub fn remove(&mut self, pos: SectionPos) {
        self.pending.remove(&pos);
    }

    pub fn contains(&self, pos: SectionPos) -> bool {
        self.pending.contains(&pos)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Pop up to `max` sections, those nearest the load centre column first.
    /// Meshing is idempotent, so the order is a priority, not a contract.
    ///
    /// The backlog can be thousands while streaming, yet only `max` (≈128) come out, so
    /// this avoids the old full `O(d log d)` sort: a partial select pulls the nearest
    /// `max` in `O(d)`, then only those few are sorted for priority order.
    fn pop_nearest_batch(&mut self, max: usize, target: Option<LoadTarget>) -> Vec<SectionPos> {
        if max == 0 || self.pending.is_empty() {
            return Vec::new();
        }
        self.scratch.clear();
        self.scratch.extend(self.pending.iter().copied());
        let n = self.scratch.len();
        let take = max.min(n);
        if let Some(t) = target {
            let key = |p: &SectionPos| -> i64 { t.section_priority_key(*p) };
            if take < n {
                self.scratch.select_nth_unstable_by_key(take, key);
            }
            self.scratch[..take].sort_unstable_by_key(key);
        }
        let result: Vec<SectionPos> = self.scratch[..take].to_vec();
        for pos in &result {
            self.pending.remove(pos);
        }
        result
    }
}

impl World {
    /// Drain finished meshes and submit newly-dirty sections to the off-thread mesh
    /// pool, capped per frame. The render thread never builds a mesh here — it only
    /// snapshots a section + its neighbourhood (cheap) and drains results — so a heavy
    /// streaming frame can't stall it.
    pub fn tick_mesh_budget(&mut self, max_per_frame: usize) {
        self.mesh_pump_frame += 1;
        self.pump_light_bakes();
        self.drain_finished_meshes();
        self.release_settled_column_meshes();
        if max_per_frame == 0 {
            return;
        }

        // Never let the pool's FIFO channel outgrow the cap: leave the rest of the backlog in
        // the nearest-first `dirty_meshes` so a just-edited section isn't stuck behind it.
        let in_flight_room = MAX_MESH_JOBS_IN_FLIGHT.saturating_sub(self.mesh_jobs_in_flight);
        if in_flight_room == 0 {
            return;
        }
        let target_jobs = max_per_frame
            .max(MIN_MESH_JOBS_PER_PUMP)
            .min(in_flight_room);
        let candidate_cap = target_jobs.saturating_mul(CANDIDATE_SCAN_PER_MESH_JOB);
        if self.vis_dirty {
            self.refresh_deep_visibility();
        }
        let target = self.last_load_target;
        let candidates = self.dirty_meshes.pop_nearest_batch(candidate_cap, target);
        let mut submitted = 0usize;
        let start = std::time::Instant::now();
        for (i, &pos) in candidates.iter().enumerate() {
            if !self.sections.contains_key(&pos) {
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
                if start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
                continue;
            }
            // Don't snapshot from stale light: a section whose 3×3×3 light isn't baked
            // yet parks outside the hot dirty queue, so the snapshot always carries final light.
            if self.request_light_dependencies(pos) {
                self.light_blocked_meshes.insert(pos);
                if start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
                continue;
            }
            if let Some(job) = self.build_mesh_job(pos) {
                let key = target.map_or(0, |t| t.section_priority_key(pos));
                self.mesh_pool.submit(key, job);
                self.mesh_jobs_in_flight += 1;
                submitted += 1;
                if submitted >= target_jobs
                    || (submitted > 0 && start.elapsed() >= MESH_SUBMIT_TIME_BUDGET)
                {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
            }
        }
    }

    /// Release the CPU mesh buffers of columns that have been upload-quiet for
    /// [`MESH_RELEASE_DELAY_FRAMES`] (stamped by `mark_column_uploaded`). The CPU
    /// copy only exists so a column repack can re-pack sibling sections; once a
    /// column settles, the copy is dead weight (~30–60 KB per meshed section) and
    /// a later repack forces a remesh of the released sections instead
    /// (`repack_forced`). Releasing never touches the GPU buffers, so a wrong
    /// "settled" verdict costs remesh work, never visible terrain.
    fn release_settled_column_meshes(&mut self) {
        if self.mesh_pump_frame % MESH_RELEASE_SWEEP_INTERVAL != 0
            || self.mesh_release_after.is_empty()
        {
            return;
        }
        let frame = self.mesh_pump_frame;
        let ripe: Vec<ChunkPos> = self
            .mesh_release_after
            .iter()
            .filter(|&(_, &after)| frame >= after)
            .map(|(&pos, _)| pos)
            .collect();
        for pos in ripe {
            self.mesh_release_after.remove(&pos);
            // Still has upload or remesh work pending: skip. The eventual upload
            // re-stamps the column via `mark_column_uploaded`.
            if self.mesh_upload_dirty_columns.contains(&pos) {
                continue;
            }
            let busy = Self::column_section_range().any(|cy| {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                self.dirty_meshes.contains(sp) || self.light_blocked_meshes.contains(&sp)
            });
            if busy {
                continue;
            }
            for cy in Self::column_section_range() {
                if let Some(mesh) = self.meshes.get_mut(&SectionPos::new(pos.cx, cy, pos.cz)) {
                    if !mesh.mesh_dirty && !mesh.is_released() {
                        mesh.release_cpu_buffers();
                    }
                }
            }
        }
    }

    /// Whether `pos` produces no visible geometry, so meshing/lighting/drawing it is pure
    /// waste: it is entirely air (the empty-sky band) and emits nothing. This is the exact
    /// counter-based case ONLY. The neighbour-plane "sealed section" skip that used to
    /// live here was removed on 2026-07-06 after playtests traced black (unlit) faces to
    /// section culling — do not reintroduce it here; see WIKI/rendering-performance.md.
    pub(super) fn section_produces_no_mesh(&self, pos: SectionPos) -> bool {
        self.sections.get(&pos).is_some_and(|s| s.is_empty_air())
    }

    /// Clear stale render output for a section that now intentionally emits no mesh.
    /// Returns true when the section is in that settled no-output state.
    pub(super) fn clear_mesh_if_section_produces_no_mesh(&mut self, pos: SectionPos) -> bool {
        if !self.section_produces_no_mesh(pos) {
            return false;
        }
        if self.remove_mesh(pos) {
            self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
        }
        self.dirty_meshes.remove(pos);
        self.light_blocked_meshes.remove(&pos);
        if let Some(s) = self.section_mut(pos) {
            s.dirty = false;
            // A mesh job may already have snapshotted this section while one of its
            // now-solid neighbours was still missing. Invalidate that exposed-border
            // result so it cannot reinstall geometry after we settle to no output.
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
        }
        true
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

    /// Install meshes the pool finished, dropping any whose section has since changed
    /// (re-edited or re-lit, so its `mesh_revision` moved) or unloaded.
    fn drain_finished_meshes(&mut self) {
        let start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < RESULT_DRAIN_MIN || start.elapsed() < RESULT_DRAIN_TIME_BUDGET {
            let Some(done) = self.mesh_pool.try_recv() else {
                break;
            };
            drained += 1;
            self.mesh_jobs_in_flight = self.mesh_jobs_in_flight.saturating_sub(1);
            let fresh = self
                .sections
                .get(&done.pos)
                .is_some_and(|s| s.mesh_revision == done.revision);
            if !fresh {
                continue;
            }
            let mut mesh = done.mesh;
            mesh.mesh_dirty = true; // needs a GPU upload on the next sync
            self.install_mesh(done.pos, mesh);
            if let Some(s) = self.section_mut(done.pos) {
                s.dirty = false;
            }
        }
    }

    /// Snapshot `pos` and its one-block-padded neighbourhood into an owned [`MeshJob`]
    /// the mesh pool can build with no access to the live world. Reads match the live
    /// neighbour accessors exactly (air / open-sky / not-loaded fallbacks), so the
    /// off-thread mesh is byte-identical to an inline one.
    fn build_mesh_job(&self, pos: SectionPos) -> Option<super::mesh_pool::MeshJob> {
        use super::mesh_pool::{
            biome_pad_idx, empty_biome, nbhd_idx27, MeshJob, NeighborSnap, BIOME_PAD,
            BIOME_PAD_RADIUS,
        };

        let center = (**self.sections.get(&pos)?).clone();
        let revision = center.mesh_revision;

        // Snapshot the 3×3×3 neighbourhood as cheap field-Arc bundles: four refcount bumps
        // each, no allocation, and no shared `Arc<Section>` — so a streaming edit/relight
        // never copy-on-write clones a section just because a mesh job is reading it. The
        // worker assembles the padded mesh buffers from these off-thread.
        let mut nbhd: [Option<NeighborSnap>; 27] = std::array::from_fn(|_| None);
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    nbhd[nbhd_idx27(dx, dy, dz)] = self
                        .sections
                        .get(&SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz))
                        .map(|s| NeighborSnap {
                            blocks: s.blocks_arc(),
                            water: s.water_arc(),
                            skylight: s.skylight_arc(),
                            blocklight: s.blocklight_arc(),
                            stair_states: sparse_state_snapshot(s.stair_states()),
                            slab_states: sparse_state_snapshot(s.slab_states()),
                        });
                }
            }
        }

        // The biome strip is tiny (20²) and lives in the (non-Arc) columns, so build it
        // here rather than carry column handles to the worker. Tint blending samples a
        // 5×5 biome window, hence the wider 2-column XZ halo. Missing edge columns fall
        // back to analytical biome generation so view-biased streaming does not leave the
        // outer visible ring permanently dirty, but the mesh never bakes biome id 0.
        let mut biome = empty_biome();
        let mut fallback_gen = None;
        let (ox, _, oz) = pos.origin_world();
        for pz in 0..BIOME_PAD {
            let wz = oz - BIOME_PAD_RADIUS + pz as i32;
            for px in 0..BIOME_PAD {
                let wx = ox - BIOME_PAD_RADIUS + px as i32;
                let cp = ChunkPos::new(
                    wx.div_euclid(chunk::SECTION_SIZE as i32),
                    wz.div_euclid(chunk::SECTION_SIZE as i32),
                );
                biome[biome_pad_idx(px, pz)] = self.columns.get(&cp).map_or_else(
                    || {
                        fallback_gen
                            .get_or_insert_with(|| {
                                crate::worldgen::driver::ChunkGenerator::new(self.seed)
                            })
                            .biome_at(wx, wz)
                            .id()
                    },
                    |c| c.biome_at(chunk::lx(wx), chunk::lz(wz)),
                );
            }
        }

        Some(MeshJob {
            pos,
            revision,
            center,
            nbhd,
            biome,
        })
    }

    /// Drain and apply finished light bakes — the light half of the pump,
    /// public so a headless server loop can keep light current with no mesh
    /// machinery attached. `tick_mesh_budget` calls this internally, so the
    /// combined/client worlds behave exactly as before.
    ///
    /// On a `ServerHeadless` world this is ALSO where rebakes are REQUESTED:
    /// edits mark light dirty into `headless_relight` (`mark_light_dirty_pos`)
    /// because the mesh pump that normally demands rebakes
    /// (`request_light_dependencies`) never runs headless. First-time bakes
    /// still come from the streamer's `flush_settled_deferred`.
    pub fn pump_light_bakes(&mut self) {
        if !self.headless_relight.is_empty() {
            debug_assert_eq!(self.role, crate::world::store::WorldRole::ServerHeadless);
            let target = self.last_load_target;
            for pos in std::mem::take(&mut self.headless_relight) {
                let bakeable = self
                    .sections
                    .get(&pos)
                    .is_some_and(|s| s.light_dirty && !s.all_opaque());
                // Deferred first-timers bake once their gen neighbourhood
                // settles (streamer-owned) — requesting here would double-bake.
                if bakeable && !self.light_deferred.contains(&pos) {
                    let key = target.map_or(0, |t| t.section_priority_key(pos));
                    self.light_bakes
                        .request(key, pos, &self.sections, &self.columns);
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
                continue;
            }
            if let Some(s) = self.section_mut(res.pos) {
                s.set_skylight(res.skylight);
                s.set_blocklight(res.blocklight);
                s.dirty = true;
                // The cached light changed, so any in-flight mesh built from the old
                // light is now stale: bump so its result is discarded and re-queue.
                s.mesh_revision = s.mesh_revision.wrapping_add(1);
            }
            self.bump_lighting_revision();
            // Headless: no meshes to relight (see `queue_dirty_mesh`).
            if self.role != crate::world::store::WorldRole::ServerHeadless {
                self.dirty_meshes.push(res.pos);
            }
        }
        self.flush_light_blocked_meshes();
    }

    /// Queue every dirty light cube a section mesh would read from its 3×3×3
    /// sampling neighbourhood. Returns true when the mesh must wait for async light.
    ///
    /// Fully-opaque neighbours are skipped: their cells are solid, so a meshed neighbour's
    /// faces are culled against them and never sample their light — baking it would be
    /// wasted, and waiting on it would stall the mesh. (Carving air in clears `all_opaque`,
    /// so it rejoins the light path then.)
    fn request_light_dependencies(&mut self, pos: SectionPos) -> bool {
        let mut waiting = false;
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self
                        .sections
                        .get(&p)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque())
                    {
                        // A deferred neighbour's first bake fires when its own
                        // neighbourhood settles (`flush_settled_deferred`); requesting
                        // it here would bake a half-landed neighbourhood and be
                        // immediately redone. Still wait on it.
                        if !self.light_deferred.contains(&p) {
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
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                    if self
                        .sections
                        .get(&p)
                        .is_some_and(|s| s.light_dirty && !s.all_opaque())
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn flush_light_blocked_meshes(&mut self) {
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

/// Owned copy of a section's sparse per-cell state map for a mesh job, `None`
/// when the section carries none (the common case — no allocation).
fn sparse_state_snapshot<T: Copy>(
    map: &std::collections::HashMap<u16, T>,
) -> Option<Box<[(u16, T)]>> {
    (!map.is_empty()).then(|| map.iter().map(|(&key, &state)| (key, state)).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::biome::Biome;
    use crate::block::Block;
    use crate::chunk::SectionPos;
    use crate::section::Section;
    use crate::world::store::LoadTarget;

    use super::{DirtyMeshQueue, World};

    fn solid_section(pos: SectionPos) -> Section {
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_slice_mut().fill(Block::Stone.id());
        section.recompute_random_tick_count();
        section.recompute_opaque_count();
        section
    }

    fn insert_solid_section(world: &mut World, pos: SectionPos) {
        world.ensure_column(pos.chunk_pos());
        world.sections.insert(pos, Arc::new(solid_section(pos)));
    }

    #[test]
    fn mesh_job_fills_missing_biome_tint_halo_from_generator() {
        let mut world = World::new(0, 0);
        let pos = SectionPos::new(0, 0, 0);
        insert_solid_section(&mut world, pos);
        for z in 0..crate::chunk::CHUNK_SZ {
            for x in 0..crate::chunk::CHUNK_SX {
                world.columns.get_mut(&pos.chunk_pos()).unwrap().set_biome(
                    x,
                    z,
                    Biome::Plains.id(),
                );
            }
        }

        let job = world
            .build_mesh_job(pos)
            .expect("missing edge columns should fall back to generated biome ids");
        assert!(
            job.biome.iter().all(|&id| id != 0),
            "mesh jobs must not bake chunk-edge tint from missing-biome id 0"
        );
    }

    #[test]
    fn light_blocked_mesh_leaves_hot_dirty_queue() {
        let mut world = World::new(0, 0);
        let pos = SectionPos::new(0, 0, 0);
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.set_block(0, 0, 0, Block::Dirt);
        world.insert_section_for_test(pos, section);

        world.tick_mesh_budget(1);

        assert!(
            world.dirty_meshes.is_empty(),
            "light-blocked meshes should not churn in the hot dirty queue"
        );
        assert!(
            world.light_blocked_meshes.contains(&pos),
            "the mesh should be parked until its light dependency finishes"
        );
    }

    #[test]
    fn dirty_mesh_priority_is_near_first_with_forward_tiebreak() {
        let target = LoadTarget::new_facing(0, 0, 0, 16, 1.0, 0.0);
        let near = SectionPos::new(0, 0, 2);
        let far_ahead = SectionPos::new(16, 0, 0);
        let front = SectionPos::new(6, 0, 0);
        let side = SectionPos::new(0, 0, 6);

        let mut queue = DirtyMeshQueue::default();
        queue.push(far_ahead);
        queue.push(near);
        assert_eq!(
            queue.pop_nearest_batch(1, Some(target)),
            vec![near],
            "near dirty meshes must beat far-ahead dirty meshes"
        );

        let mut queue = DirtyMeshQueue::default();
        queue.push(side);
        queue.push(front);
        assert_eq!(
            queue.pop_nearest_batch(1, Some(target)),
            vec![front],
            "same-distance dirty meshes in the forward cone should win"
        );
    }

    #[test]
    fn all_air_transition_removes_stale_ghost_mesh() {
        let mut world = World::new(0, 0);
        let center = SectionPos::new(0, 0, 0);
        insert_solid_section(&mut world, center);
        world.queue_dirty_mesh(center);

        world.mesh_section_blocking_for_test(center);
        assert!(
            world.meshes.get(&center).is_some_and(|m| !m.is_empty()),
            "a solid section with missing neighbours meshes its exposed border"
        );

        // Mine the section out entirely: all-air emits nothing.
        let before_revision = {
            let s = world.section_mut(center).unwrap();
            s.blocks_slice_mut().fill(Block::Air.id());
            s.recompute_random_tick_count();
            s.recompute_opaque_count();
            s.mesh_revision
        };
        assert!(
            world.clear_mesh_if_section_produces_no_mesh(center),
            "the all-air section should settle to no render output"
        );
        assert!(
            world
                .sections
                .get(&center)
                .is_some_and(|s| s.mesh_revision > before_revision),
            "settling to no-mesh must invalidate in-flight jobs built from the old blocks"
        );
        assert!(
            !world.meshes.contains_key(&center),
            "stale ghost mesh must be removed"
        );
        assert!(
            world
                .mesh_upload_dirty_columns
                .contains(&center.chunk_pos()),
            "the render column must be marked for GPU repack"
        );
    }

    #[test]
    fn buried_mixed_sections_are_not_sealed_away() {
        // Regression guard for the removed neighbour-plane "sealed section" skip: a
        // buried section between solid neighbours must still schedule mesh+light work
        // (its light is a seam-sampling dependency for meshed neighbours; skipping it
        // rendered adjoining faces black).
        let mut world = World::new(0, 0);
        let center = SectionPos::new(0, 0, 0);
        insert_solid_section(&mut world, center);
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            insert_solid_section(
                &mut world,
                SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz),
            );
        }
        assert!(
            !world.section_produces_no_mesh(center),
            "only all-air sections settle to no output; enclosure must not seal a section"
        );
    }
}
