use rustc_hash::FxHashSet;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

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
const MAX_MESH_JOBS_IN_FLIGHT: usize = 16;
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
pub(super) struct DirtyMeshQueue {
    pending: FxHashSet<SectionPos>,
    /// Entries cache their priority once. Removal is lazy: `pending` remains the
    /// source of truth and stale heap rows are skipped when popped.
    heap: BinaryHeap<Reverse<(i64, i32, i32, i32)>>,
    target: Option<LoadTarget>,
}

impl Default for DirtyMeshQueue {
    fn default() -> Self {
        Self {
            pending: FxHashSet::default(),
            heap: BinaryHeap::new(),
            target: None,
        }
    }
}

impl DirtyMeshQueue {
    fn entry(target: Option<LoadTarget>, pos: SectionPos) -> Reverse<(i64, i32, i32, i32)> {
        Reverse((
            target.map_or(0, |t| t.section_priority_key(pos)),
            pos.cx,
            pos.cy,
            pos.cz,
        ))
    }

    fn rebuild(&mut self, target: Option<LoadTarget>) {
        self.target = target;
        self.heap.clear();
        self.heap
            .extend(self.pending.iter().copied().map(|p| Self::entry(target, p)));
    }

    pub fn push(&mut self, pos: SectionPos) {
        if self.pending.insert(pos) {
            self.heap.push(Self::entry(self.target, pos));
        }
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
    /// The heap is rebuilt only when the quantized load target changes. Ordinary
    /// frames pop `O(max log d)` work without copying or scanning the backlog.
    fn pop_nearest_batch(&mut self, max: usize, target: Option<LoadTarget>) -> Vec<SectionPos> {
        if max == 0 || self.pending.is_empty() {
            return Vec::new();
        }
        if self.target != target || self.heap.len() > self.pending.len().saturating_mul(4) + 1024 {
            self.rebuild(target);
        }
        let mut result = Vec::with_capacity(max.min(self.pending.len()));
        while result.len() < max {
            let Some(Reverse((_, cx, cy, cz))) = self.heap.pop() else {
                break;
            };
            let pos = SectionPos::new(cx, cy, cz);
            if self.pending.remove(&pos) {
                result.push(pos);
            }
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
        let frame_start = std::time::Instant::now();
        self.mesh_pump_frame += 1;
        self.pump_light_bakes();
        self.drain_finished_meshes();
        self.release_settled_column_meshes();
        if max_per_frame == 0 || frame_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
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
        // Ingest can raise this every frame while thousands of sections arrive.
        // One refresh per eight frames keeps the O(deep) BFS bounded; parked
        // sections re-enter automatically on the next refresh.
        if self.vis_dirty && self.mesh_pump_frame.is_multiple_of(8) {
            self.refresh_deep_visibility();
        }
        if frame_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
            return;
        }
        let target = self.last_load_target;
        let candidates = self.dirty_meshes.pop_nearest_batch(candidate_cap, target);
        let mut submitted = 0usize;
        for (i, &pos) in candidates.iter().enumerate() {
            if submitted > 0 && frame_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
                for &rest in &candidates[i..] {
                    self.dirty_meshes.push(rest);
                }
                break;
            }
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
                if frame_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
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
                if frame_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET {
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
                    || (submitted > 0 && frame_start.elapsed() >= MESH_SUBMIT_TIME_BUDGET)
                {
                    for &rest in &candidates[i + 1..] {
                        self.dirty_meshes.push(rest);
                    }
                    break;
                }
            }
        }
    }

    /// Same-frame remesh of a LOCAL PREDICTED edit's footprint. The async
    /// pipeline needs two pump round-trips (light bake, then mesh build)
    /// before an edit reaches the GPU — a predicted break lingered on screen
    /// for 2–3 frames after its sound/burst played. This builds the touched
    /// sections NOW on the calling thread with the exact worker code
    /// (`mesh_pool::build_inline`), bounded to the footprint's own sections
    /// plus face-adjacent ones across any boundary plane a cell sits on
    /// (their culled faces appear/vanish with the edit). Diagonal-only
    /// neighbours change shading (AO / smooth light), never face existence —
    /// the ordinary async remesh corrects them a couple of frames later.
    ///
    /// Light is sampled from the CURRENT cubes, raised by a one-step flood
    /// estimate at each opened cell so newly exposed faces don't flash the
    /// removed block's stale darkness. Nothing here touches `light_dirty`,
    /// `dirty_meshes`, or the section `dirty` flag: the edit's async
    /// bake → remesh still follows and supersedes this provisional mesh.
    pub fn remesh_edited_cells_inline(&mut self, cells: &[crate::mathh::IVec3]) {
        const EDGE: i32 = chunk::SECTION_SIZE as i32 - 1;
        fn touch(touched: &mut Vec<SectionPos>, pos: SectionPos) {
            if !touched.contains(&pos) {
                touched.push(pos);
            }
        }
        let mut touched: Vec<SectionPos> = Vec::with_capacity(4);
        for &cell in cells {
            self.seed_open_cell_light_estimate(cell);
            let Some(center) = SectionPos::from_world(cell.x, cell.y, cell.z) else {
                continue;
            };
            touch(&mut touched, center);
            for (l, lo, hi) in [
                (cell.x & 0x0F, (-1, 0, 0), (1, 0, 0)),
                (cell.y & 0x0F, (0, -1, 0), (0, 1, 0)),
                (cell.z & 0x0F, (0, 0, -1), (0, 0, 1)),
            ] {
                let d = match l {
                    0 => lo,
                    EDGE => hi,
                    _ => continue,
                };
                touch(
                    &mut touched,
                    SectionPos::new(center.cx + d.0, center.cy + d.1, center.cz + d.2),
                );
            }
        }
        for pos in touched {
            if !self.sections.contains_key(&pos) {
                continue;
            }
            // Mined the section's last block: settle to no output (removes
            // the ghost mesh) inline too. Hidden/sealed parking is skipped on
            // purpose — a player edit is always inside the visible near ring.
            if self.clear_mesh_if_section_produces_no_mesh(pos) {
                continue;
            }
            let Some(job) = self.build_mesh_job(pos) else {
                continue;
            };
            let Some(mut mesh) = super::mesh_pool::build_inline(job) else {
                continue;
            };
            mesh.mesh_dirty = true; // GPU upload on this frame's sync
            self.install_mesh(pos, mesh);
        }
    }

    /// Raise a just-opened cell's cached light to its one-flood-step estimate
    /// (max of the six orthogonal neighbours minus one step on the x2 scale;
    /// a full-sky cell above seeds full sky — the column just opened to the
    /// sky). Raise-only and revision-free via [`Section::seed_light_estimate`]:
    /// the pending authoritative bake replaces the cubes wholesale.
    fn seed_open_cell_light_estimate(&mut self, cell: crate::mathh::IVec3) {
        use crate::chunk::SKY_FULL;
        use crate::mathh::IVec3;
        let block = crate::block::Block::from_id(self.chunk_block(cell.x, cell.y, cell.z));
        if block.is_opaque() {
            return;
        }
        let mut sky = 0u8;
        let mut blk = 0u8;
        for d in [
            IVec3::new(1, 0, 0),
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 1, 0),
            IVec3::new(0, -1, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(0, 0, -1),
        ] {
            let n = cell + d;
            let nsky = self.skylight_at_world(n.x, n.y, n.z);
            sky = sky.max(if d.y == 1 && nsky == SKY_FULL {
                SKY_FULL
            } else {
                nsky.saturating_sub(2)
            });
            blk = blk.max(self.blocklight_at_world(n.x, n.y, n.z).saturating_sub(2));
        }
        if sky == 0 && blk == 0 {
            return;
        }
        if let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(cell.x, cell.y, cell.z) {
            section.seed_light_estimate(lx, ly, lz, sky, blk);
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
        if !self
            .mesh_pump_frame
            .is_multiple_of(MESH_RELEASE_SWEEP_INTERVAL)
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
    /// section culling — do not reintroduce it here.
    pub(super) fn section_produces_no_mesh(&self, pos: SectionPos) -> bool {
        self.sections.get(&pos).is_some_and(|s| s.is_empty_air())
    }

    /// Exact future-work skip: every adjoining plane is a loaded, fully opaque
    /// wall, so no outside sightline or emitted boundary face can reach this
    /// section. A nearby player overrides the proof because they may already be
    /// inside an enclosed cave. Generated summaries are deliberately not trusted
    /// for saved terrain.
    pub(super) fn section_sealed_by_loaded_neighbors(&self, pos: SectionPos) -> bool {
        if self.last_load_target.is_some() && self.near_load_center(pos) {
            return false;
        }
        [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ]
        .into_iter()
        .all(|(dx, dy, dz)| {
            self.sections
                .get(&SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz))
                .is_some_and(|s| s.face_plane_fully_opaque(-dx, -dy, -dz))
        })
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
        self.hidden_parked.remove(&pos);
        self.sealed_parked.remove(&pos);
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
            if self
                .mesh_job_cancels
                .get(&done.pos)
                .is_some_and(|current| current.same_job(&done.cancel))
            {
                self.mesh_job_cancels.remove(&done.pos);
            }
            let Some(mut mesh) = done.mesh else {
                continue;
            };
            let fresh = self
                .sections
                .get(&done.pos)
                .is_some_and(|s| s.mesh_revision == done.revision);
            if !fresh {
                continue;
            }
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

        // Every live column carries the complete tint halo, captured by the
        // column-generation worker or replicated by the server. Hand-built test
        // worlds fall back to loaded column facts only; live submission never runs
        // analytical worldgen on this thread.
        let biome = self
            .column_gen
            .get(&pos.chunk_pos())
            .map(|col| col.mesh_biome())
            .or_else(|| self.column_biome_halos.get(&pos.chunk_pos()).cloned())
            .unwrap_or_else(|| {
                let mut halo = empty_biome();
                let data = Arc::make_mut(&mut halo);
                let (ox, _, oz) = pos.origin_world();
                for pz in 0..BIOME_PAD {
                    let wz = oz - BIOME_PAD_RADIUS + pz as i32;
                    for px in 0..BIOME_PAD {
                        let wx = ox - BIOME_PAD_RADIUS + px as i32;
                        if let Some(col) = self.columns.get(&ChunkPos::new(
                            wx.div_euclid(chunk::SECTION_SIZE as i32),
                            wz.div_euclid(chunk::SECTION_SIZE as i32),
                        )) {
                            data[biome_pad_idx(px, pz)] =
                                col.biome_at(chunk::lx(wx), chunk::lz(wz));
                        }
                    }
                }
                halo
            });

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
            if let Some(s) = self.section_mut(res.pos) {
                s.set_skylight(res.skylight);
                s.set_blocklight(res.blocklight);
                s.dirty = true;
                // The cached light changed, so any in-flight mesh built from the old
                // light is now stale: bump so its result is discarded and re-queue.
                s.mesh_revision = s.mesh_revision.wrapping_add(1);
            }
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
                        && !self.section_sealed_by_loaded_neighbors(p)
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
                        && !self.section_sealed_by_loaded_neighbors(p)
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

    use crate::block::Block;
    use crate::chunk::{SectionPos, SECTION_VOLUME};
    use crate::section::Section;
    use crate::world::store::LoadTarget;

    use super::{DirtyMeshQueue, World};

    fn solid_section(pos: SectionPos) -> Section {
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_slice_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        section
    }

    fn insert_solid_section(world: &mut World, pos: SectionPos) {
        world.ensure_column(pos.chunk_pos());
        world.sections.insert(pos, Arc::new(solid_section(pos)));
    }

    fn insert_sealed_cavity(world: &mut World, center: SectionPos) {
        let mut cavity = solid_section(center);
        cavity.set_block(8, 8, 8, Block::Air);
        world.insert_section_for_test(center, cavity);
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            insert_solid_section(
                world,
                SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz),
            );
        }
    }

    #[test]
    fn mesh_job_uses_column_generated_biome_tint_halo() {
        let mut world = World::new(0, 0);
        let pos = SectionPos::new(0, 0, 0);
        insert_solid_section(&mut world, pos);
        let gen =
            crate::worldgen::driver::ChunkGenerator::new(0).generate_column_gen(pos.cx, pos.cz);
        world.column_gen.insert(pos.chunk_pos(), Arc::new(gen));

        let job = world
            .build_mesh_job(pos)
            .expect("the center column carries its complete tint halo");
        assert!(
            job.biome.iter().all(|&id| id != 0),
            "mesh jobs must not bake chunk-edge tint from missing-biome id 0"
        );
    }

    #[test]
    fn stale_rejected_light_bake_requests_a_rebake() {
        // A revision bump while a bake is in flight (an edit, a neighbour
        // landing) makes the result stale. Its rejection is the only moment
        // the pending slot is clear again — every request during the flight
        // was dedup-dropped — so without an immediate re-request the section
        // wedges light-dirty and every mesh sampling it parks forever.
        let mut world = World::new(0, 1);
        let pos = SectionPos::new(0, 0, 0);
        let mut section = solid_section(pos);
        section.set_block(8, 8, 8, Block::Air);
        world.insert_section_for_test(pos, section);
        assert!(world.sections[&pos].light_dirty, "fixture: bake wanted");

        world
            .light_bakes
            .request(0, pos, &world.sections, &world.columns);
        // Invalidate the in-flight bake exactly as an edit / landing does. The
        // result is drained on this thread, so the bump always beats it.
        world.mark_light_dirty_pos(pos);

        for _ in 0..2500 {
            world.pump_light_bakes();
            if !world.sections[&pos].light_dirty {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("stale-rejected bake was never re-requested: section wedged light-dirty");
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
    fn dirty_mesh_priority_is_near_first() {
        let target = LoadTarget::new(0, 0, 0, 16);
        let near = SectionPos::new(0, 0, 2);
        let far = SectionPos::new(16, 0, 0);

        let mut queue = DirtyMeshQueue::default();
        queue.push(far);
        queue.push(near);
        assert_eq!(
            queue.pop_nearest_batch(1, Some(target)),
            vec![near],
            "near dirty meshes must beat far dirty meshes"
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
    fn loaded_opaque_neighbour_planes_seal_future_mesh_work() {
        // Only exact loaded planes may seal; generated summaries can disagree
        // with saved/player-carved terrain.
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
            world.section_sealed_by_loaded_neighbors(center),
            "six exact opaque neighbour planes make future mesh work invisible"
        );
    }

    #[test]
    fn sealed_section_around_player_still_meshes_and_remeshes() {
        let mut world = World::new(0, 4);
        let center = SectionPos::new(0, 0, 0);
        insert_sealed_cavity(&mut world, center);
        world.last_load_target = Some(LoadTarget::new(0, 0, 0, 4));

        assert!(
            !world.section_sealed_by_loaded_neighbors(center),
            "a player can already be inside an otherwise sealed underground section"
        );
        world.mesh_section_blocking_for_test(center);
        assert!(
            world
                .meshes
                .get(&center)
                .is_some_and(|mesh| !mesh.is_empty()),
            "the internal cavity walls must mesh around the player"
        );

        let before = world.mesh_upload_revisions[&center.chunk_pos()];
        world
            .section_mut(center)
            .unwrap()
            .set_block(9, 8, 8, Block::Air);
        world.queue_dirty_mesh(center);
        world.mesh_section_blocking_for_test(center);
        assert!(
            world.mesh_upload_revisions[&center.chunk_pos()] > before,
            "an edit inside the sealed section must install a fresh mesh"
        );
    }

    #[test]
    fn far_sealed_section_requeues_when_player_approaches() {
        let mut world = World::new(0, 16);
        let center = SectionPos::new(0, 0, 0);
        insert_sealed_cavity(&mut world, center);
        world
            .section_mut(center)
            .unwrap()
            .set_skylight(Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice()));
        world.last_load_target = Some(LoadTarget::new(8, 0, 0, 16));

        world.tick_mesh_budget(1);
        assert!(world.sealed_parked.contains(&center));
        assert!(!world.meshes.contains_key(&center));

        world.last_load_target = Some(LoadTarget::new(0, 0, 0, 16));
        world.vis_dirty = true;
        world.refresh_deep_visibility();
        assert!(!world.sealed_parked.contains(&center));
        assert!(world.dirty_meshes.contains(center));
        world.mesh_section_blocking_for_test(center);
        assert!(world.meshes.contains_key(&center));
    }

    #[test]
    fn local_edit_inline_remesh_lands_without_a_pump() {
        // A predicted break must reach the GPU-upload set the same call, not
        // after the async light-bake + mesh-pool round-trips — and it must
        // NOT consume the pending async pipeline (the light-fresh remesh is
        // still owed).
        let mut world = World::new(0, 4);
        let pos = SectionPos::new(0, 0, 0);
        insert_solid_section(&mut world, pos);
        world.last_load_target = Some(LoadTarget::new(0, 0, 0, 4));
        world.queue_dirty_mesh(pos);
        world.mesh_section_blocking_for_test(pos);
        let before = world.mesh_upload_revisions[&pos.chunk_pos()];

        assert!(world.set_block_world(8, 8, 8, Block::Air));
        world.remesh_edited_cells_inline(&[crate::mathh::IVec3::new(8, 8, 8)]);

        assert!(
            world.mesh_upload_revisions[&pos.chunk_pos()] > before,
            "the edit must be render-visible with zero budget pumps"
        );
        assert!(
            world.dirty_meshes.contains(pos),
            "the async light-fresh remesh must still be owed"
        );
        assert!(
            world.sections[&pos].light_dirty,
            "the inline path must not satisfy or cancel the pending rebake"
        );
    }

    #[test]
    fn boundary_edit_inline_remeshes_the_face_adjacent_neighbour() {
        // Carving a cell on the A|B section plane exposes B's culled face;
        // if only A remeshed inline, the hole would show hollow terrain for
        // the async round-trip. Face-adjacent sections must land together.
        let mut world = World::new(0, 4);
        let a = SectionPos::new(0, 0, 0);
        let b = SectionPos::new(1, 0, 0);
        insert_solid_section(&mut world, a);
        insert_solid_section(&mut world, b);
        world.last_load_target = Some(LoadTarget::new(0, 0, 0, 4));
        for &pos in &[a, b] {
            world.queue_dirty_mesh(pos);
            world.mesh_section_blocking_for_test(pos);
        }
        let before_b = world.mesh_upload_revisions[&b.chunk_pos()];

        assert!(world.set_block_world(15, 8, 8, Block::Air));
        world.remesh_edited_cells_inline(&[crate::mathh::IVec3::new(15, 8, 8)]);

        assert!(
            world.mesh_upload_revisions[&b.chunk_pos()] > before_b,
            "the face-adjacent neighbour must remesh in the same call"
        );
    }

    #[test]
    fn forced_repack_remesh_bypasses_sealed_parking() {
        let mut world = World::new(0, 16);
        let center = SectionPos::new(0, 0, 0);
        insert_sealed_cavity(&mut world, center);
        world
            .section_mut(center)
            .unwrap()
            .set_skylight(Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice()));
        world.last_load_target = Some(LoadTarget::new(8, 0, 0, 16));
        world.repack_forced.insert(center);
        world.queue_dirty_mesh(center);

        world.mesh_section_blocking_for_test(center);
        assert!(world.meshes.contains_key(&center));
        assert!(!world.repack_forced.contains(&center));
        assert!(!world.sealed_parked.contains(&center));
    }
}
