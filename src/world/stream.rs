use rustc_hash::FxHashSet;
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{
    section_idx, ChunkPos, SectionPos, SEA_LEVEL, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::entity::DroppedItem;
use crate::mathh::IVec3;
use crate::mob::SavedMob;
use crate::section::Section;
use crate::worker::{GenJob, GenOutput};
use crate::worldgen::driver::ColumnGen;

use super::store::{
    LoadTarget, World, FORWARD_LOAD_DOT_MIN, OMNI_LOAD_RADIUS, VERTICAL_LOAD_RADIUS,
};

// Used only by the column-era test/fixture helper `split_generated_column`.
#[cfg(test)]
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
#[cfg(test)]
use crate::column::Column;

const SURFACE_WINDOW_BELOW: i32 = 2;
const SURFACE_WINDOW_ABOVE: i32 = 1;
const HORIZONTAL_KEEP_SLACK: i32 = 2;
/// Keep worldgen useful under fast flight by bounding queued-but-unstarted column
/// jobs. The shared pool is priority-ordered (nearest first), so — unlike the old
/// FIFO channel these caps were sized for — far columns can no longer delay near
/// ones; the caps now only bound wasted work on columns the player outruns
/// (pruned from `pending`, their results discarded on drain).
const MAX_PENDING_COLUMN_GEN_JOBS: usize = 192;
const MAX_COLUMN_GEN_SUBMITS_PER_TARGET: usize = 64;
/// Drain finished worldgen by TIME with a count floor: installs are cheap (map
/// insert + classify), so a fixed count frame-quantized big bursts (a whole r=20
/// disc took ~100 frames just to drain at 128/frame), while the budget still keeps
/// one frame from installing an unbounded burst and starving rendering.
const GEN_DRAIN_MIN_PER_POLL: usize = 128;
const GEN_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(1_000);

/// A saved section read back from disk, awaiting overlay over its generated column:
/// the decoded `Section` plus the item entities and mobs that rode in its record.
pub(super) type LoadedOverlay = (Section, Vec<DroppedItem>, Vec<SavedMob>);

/// A section install the per-frame streamer performed, buffered for the tick-side
/// event bus (`section_generated` / `section_loaded`): handlers must never run
/// from per-frame code, so `poll` only records and the next game tick dispatches.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    /// A freshly generated section was installed.
    Generated(SectionPos),
    /// A saved (player-modified) section read from disk was overlaid over its
    /// generated base.
    Loaded(SectionPos),
}

impl World {
    /// Update the streamed region around the player's SECTION `(cam_chunk_x, cam_chunk_y,
    /// cam_chunk_z)`. The world streams a flattened cylinder: a Euclidean horizontal disc
    /// of columns, each loaded only across a vertical window of sections around the player
    /// (see [`VERTICAL_LOAD_RADIUS`]). Generation is per 16³ section, prioritised by 3D
    /// distance — "worldgen closest to the player" — so the deep underground / high sky a
    /// far column doesn't need is never generated until the player approaches it (room for
    /// caves below y=0). Scans are gated to player-section / render-distance changes; call
    /// `poll` every frame to keep ingesting worker results.
    pub fn update_load(&mut self, cam_chunk_x: i32, cam_chunk_y: i32, cam_chunk_z: i32) {
        let target = LoadTarget::new(cam_chunk_x, cam_chunk_y, cam_chunk_z, self.render_dist);
        self.update_load_target(target);
    }

    /// Camera-facing streaming: an omnidirectional safety ring around the player plus a
    /// broad forward outer sector. This is the live game path; the full-disc
    /// `update_load` remains for tools/tests that need deterministic whole-area loads.
    pub fn update_load_facing(
        &mut self,
        cam_chunk_x: i32,
        cam_chunk_y: i32,
        cam_chunk_z: i32,
        forward_x: f32,
        forward_z: f32,
    ) {
        let target = LoadTarget::new_facing(
            cam_chunk_x,
            cam_chunk_y,
            cam_chunk_z,
            self.render_dist,
            forward_x,
            forward_z,
        );
        self.update_load_target(target);
    }

    fn update_load_target(&mut self, target: LoadTarget) {
        if self.last_load_target == Some(target) {
            self.request_missing_columns(target);
            return;
        }
        let prev = self.last_load_target;
        self.last_load_target = Some(target);
        // The player ring and disc edge moved; deep-visibility must re-evaluate.
        self.vis_dirty = true;
        let vertical_moved = prev.is_none_or(|p| p.center_cy != target.center_cy);
        let horizontal_keep_changed =
            prev.is_none_or(|p| p.center != target.center || p.render_dist != target.render_dist);

        self.prune_stale_column_requests(target);
        self.request_missing_columns(target);
        // `request_wanted_sections` re-scans EVERY loaded column's whole vertical window.
        // That full scan only changes existing wanted columns when the vertical centre
        // moves. Horizontal/sector changes can still make an already-generated column
        // newly wanted, so scan only that entering subset instead of every column in
        // the new cone.
        if vertical_moved {
            match prev {
                Some(p) => self.request_vertical_delta_sections(p, target),
                None => self.request_wanted_sections(target),
            }
        }
        if !vertical_moved {
            if let Some(prev) = prev {
                if prev.center != target.center
                    || prev.render_dist != target.render_dist
                    || prev.view_sector != target.view_sector
                {
                    self.request_newly_wanted_sections(prev, target);
                }
            }
        }
        if horizontal_keep_changed || vertical_moved {
            self.unload_far(target, vertical_moved);
        }
    }

    /// The vertical section-`cy` window around the player, clamped to the world range.
    /// `slack` widens it (used by unload for hysteresis so a section doesn't thrash on
    /// the boundary).
    fn vertical_window(center_cy: i32, slack: i32) -> std::ops::RangeInclusive<i32> {
        let center_cy = center_cy.clamp(SECTION_MIN_CY, SECTION_MAX_CY);
        let r = VERTICAL_LOAD_RADIUS + slack;
        (center_cy - r).max(SECTION_MIN_CY)..=(center_cy + r).min(SECTION_MAX_CY)
    }

    /// A surface/content retention band for a generated column. This is intentionally
    /// independent from the player's current section: spectator flight far above the
    /// world should not evict the terrain stack underneath a still-visible column.
    pub(super) fn surface_window_for_column(
        col: &ColumnGen,
        slack: i32,
    ) -> std::ops::RangeInclusive<i32> {
        let (surf_min, _) = col.surf_range();
        let bottom_y = surf_min.max(SEA_LEVEL);
        let top_y = col.content_top().max(SEA_LEVEL);
        let lo = bottom_y.div_euclid(SECTION_SIZE as i32) - SURFACE_WINDOW_BELOW - slack;
        let hi = top_y.div_euclid(SECTION_SIZE as i32) + SURFACE_WINDOW_ABOVE + slack;
        lo.max(SECTION_MIN_CY)..=hi.min(SECTION_MAX_CY)
    }

    /// Player-centred vertical window plus the column's surface/content band.
    /// UNORDERED (duplicates removed in-place): every consumer re-orders by its own
    /// submission priority key, so sorting here was pure per-column waste.
    fn wanted_section_cys(col: &ColumnGen, center_cy: i32, slack: i32) -> Vec<i32> {
        let mut out: Vec<i32> = Self::vertical_window(center_cy, slack).collect();
        for cy in Self::surface_window_for_column(col, slack) {
            if !out.contains(&cy) {
                out.push(cy);
            }
        }
        out
    }

    fn wanted_section_cys_for_column(
        &self,
        pos: ChunkPos,
        col: &ColumnGen,
        center_cy: i32,
        slack: i32,
    ) -> Vec<i32> {
        let mut out = Self::wanted_section_cys(col, center_cy, slack);
        if let Some(save) = self.save.as_ref() {
            for sp in save.manifest_sections_in_column(pos) {
                if !out.contains(&sp.cy) {
                    out.push(sp.cy);
                }
            }
        }
        out
    }

    fn column_shape_key(target: LoadTarget, pos: ChunkPos) -> (i32, i32, i32) {
        (
            pos.cx - target.center.cx,
            pos.cz - target.center.cz,
            target.render_dist.max(0),
        )
    }

    fn column_in_shape(target: LoadTarget, pos: ChunkPos, slack: i32, dot_min: f32) -> bool {
        let (dx, dz, r) = Self::column_shape_key(target, pos);
        let r = r + slack;
        let d2 = dx * dx + dz * dz;
        if d2 > r * r {
            return false;
        }
        let Some((fx, fz)) = target.view_dir() else {
            return true;
        };
        let omni = (OMNI_LOAD_RADIUS + slack).min(r).max(0);
        if d2 <= omni * omni {
            return true;
        }
        let dist = (d2 as f32).sqrt();
        dist > 0.0 && (dx as f32 * fx + dz as f32 * fz) / dist >= dot_min
    }

    /// `pub(super)` for the sim guard: an absent column that is wanted under the
    /// current target counts as in-flight, not as never-coming.
    pub(super) fn column_wanted(target: LoadTarget, pos: ChunkPos) -> bool {
        Self::column_in_shape(target, pos, 0, FORWARD_LOAD_DOT_MIN)
    }

    fn column_kept(target: LoadTarget, pos: ChunkPos) -> bool {
        let (dx, dz, r) = Self::column_shape_key(target, pos);
        let keep = r + HORIZONTAL_KEEP_SLACK;
        dx * dx + dz * dz <= keep * keep
    }

    /// Submit the (heavy, once-per-column) `ColumnGen` job for every in-radius column we
    /// have neither loaded nor queued, NEAREST-FIRST so the player's surroundings resolve
    /// first. Each landed column then drives its own per-section jobs (`poll`).
    fn request_missing_columns(&mut self, target: LoadTarget) {
        let submit_limit = MAX_COLUMN_GEN_SUBMITS_PER_TARGET
            .min(MAX_PENDING_COLUMN_GEN_JOBS.saturating_sub(self.pending.len()));
        if submit_limit == 0 {
            return;
        }
        let center = target.center;
        let r = target.render_dist;
        let mut missing: Vec<(i64, ChunkPos)> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                let pos = ChunkPos::new(center.cx + dx, center.cz + dz);
                if !Self::column_wanted(target, pos) {
                    continue;
                }
                if self.column_gen.contains_key(&pos) || self.pending.contains_key(&pos) {
                    continue;
                }
                missing.push((target.column_priority_key(pos), pos));
            }
        }
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.worker.submit(
                priority,
                GenJob::Column {
                    pos,
                    seed: self.seed,
                },
            );
            self.pending.insert(pos, ());
        }
    }

    fn prune_stale_column_requests(&mut self, target: LoadTarget) {
        self.pending
            .retain(|pos, _| Self::column_wanted(target, *pos));
    }

    /// Across every loaded column in the horizontal radius, submit per-section gen jobs
    /// for the wanted-but-absent sections of the vertical window, globally NEAREST-FIRST
    /// in 3D. Run when the player's section moves (the window shifts); newly-arrived
    /// columns are handled directly in `poll` via [`request_sections_for_column`].
    fn request_wanted_sections(&mut self, target: LoadTarget) {
        self.request_wanted_sections_matching(target, |_| true);
    }

    /// Vertical-crossing section requests. Columns already wanted under `prev` had
    /// their full window + surface band + manifest requested when they entered (and
    /// their player-window edge on every crossing since), so only the cys ENTERING
    /// the player window this move need checking — plus saved manifest sections,
    /// which stream in regardless of the vertical window (sky builds). Columns just
    /// entering the wanted shape still get the full per-column window build. This
    /// turns the per-crossing O(columns × window) rescan into O(columns × Δ).
    fn request_vertical_delta_sections(&mut self, prev: LoadTarget, target: LoadTarget) {
        let prev_window = Self::vertical_window(prev.center_cy, 0);
        let mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)> = Vec::new();
        let mut cys: Vec<i32> = Vec::new();
        for (pos, col) in &self.column_gen {
            if !Self::column_wanted(target, *pos) {
                continue;
            }
            cys.clear();
            if Self::column_wanted(prev, *pos) {
                cys.extend(
                    Self::vertical_window(target.center_cy, 0)
                        .filter(|cy| !prev_window.contains(cy)),
                );
            } else {
                cys.extend(Self::vertical_window(target.center_cy, 0));
                for cy in Self::surface_window_for_column(col, 0) {
                    if !cys.contains(&cy) {
                        cys.push(cy);
                    }
                }
            }
            if let Some(save) = self.save.as_ref() {
                for sp in save.manifest_sections_in_column(*pos) {
                    if !cys.contains(&sp.cy) {
                        cys.push(sp.cy);
                    }
                }
            }
            let content_top = col.content_top();
            for &cy in &cys {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if self.sections.contains_key(&sp) || self.pending_sections.contains(&sp) {
                    continue;
                }
                if self.skip_empty_sky_section(sp, content_top) {
                    continue;
                }
                wanted.push((target.section_priority_key(sp), sp, col.clone()));
            }
        }
        wanted.sort_by_key(|(priority, _, _)| *priority);
        for (priority, sp, col) in wanted {
            self.submit_section_job(priority, sp, col);
        }
    }

    fn request_newly_wanted_sections(&mut self, prev: LoadTarget, target: LoadTarget) {
        self.request_wanted_sections_matching(target, |pos| !Self::column_wanted(prev, pos));
    }

    fn request_wanted_sections_matching(
        &mut self,
        target: LoadTarget,
        mut include_column: impl FnMut(ChunkPos) -> bool,
    ) {
        let center_cy = target.center_cy;
        let mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)> = Vec::new();
        for (pos, col) in &self.column_gen {
            if !Self::column_wanted(target, *pos) || !include_column(*pos) {
                continue;
            }
            for cy in self.wanted_section_cys_for_column(*pos, col, center_cy, 0) {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if self.sections.contains_key(&sp) || self.pending_sections.contains(&sp) {
                    continue;
                }
                if self.skip_empty_sky_section(sp, col.content_top()) {
                    continue;
                }
                wanted.push((target.section_priority_key(sp), sp, col.clone()));
            }
        }
        wanted.sort_by_key(|(priority, _, _)| *priority);
        for (priority, sp, col) in wanted {
            self.submit_section_job(priority, sp, col);
        }
    }

    /// Submit per-section gen jobs for one freshly-loaded column's vertical window
    /// (nearest the player's `cy` first), so a column starts filling the moment its
    /// shared data lands without waiting for the next `update_load`.
    fn request_sections_for_column(&mut self, pos: ChunkPos, target: LoadTarget) {
        let Some(col) = self.column_gen.get(&pos).cloned() else {
            return;
        };
        let mut wanted: Vec<(i64, SectionPos)> = Vec::new();
        let content_top = col.content_top();
        for cy in self.wanted_section_cys_for_column(pos, &col, target.center_cy, 0) {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if self.sections.contains_key(&sp) || self.pending_sections.contains(&sp) {
                continue;
            }
            if self.skip_empty_sky_section(sp, content_top) {
                continue;
            }
            // The full 3D key (not just dcy²): these compete in the shared pool
            // against other columns' sections, so the key must be globally comparable.
            wanted.push((target.section_priority_key(sp), sp));
        }
        wanted.sort_by_key(|(key, _)| *key);
        for (key, sp) in wanted {
            self.submit_section_job(key, sp, col.clone());
        }
    }

    /// Whether `sp` can be left ungenerated: it sits entirely above its column's content
    /// (provably all-air sky) AND the save holds no player edit there. Absent sky sections
    /// read as air with full skylight, and building into the sky materializes the section
    /// on write — so skipping them costs the common case nothing while still streaming any
    /// sky structure the player saved. Halving the loaded section count this way cuts gen,
    /// meshing, AND lighting, since each scales with the number of loaded sections.
    fn skip_empty_sky_section(&self, sp: SectionPos, content_top: i32) -> bool {
        (sp.cy * SECTION_SIZE as i32) > content_top
            && !self.save.as_ref().is_some_and(|s| s.manifest_contains(sp))
    }

    /// Queue one section's gen job and, paired with it, ask the save thread for that
    /// section's saved (player-modified) record if one exists — so the disk overlay
    /// lands after the generated base and wins (`apply_pending_overlays`).
    fn submit_section_job(&mut self, key: i64, sp: SectionPos, col: Arc<ColumnGen>) {
        self.worker.submit(
            key,
            GenJob::Section {
                sp,
                col,
                seed: self.seed,
            },
        );
        self.pending_sections.insert(sp);
        if let Some(save) = self.save.as_ref() {
            if save.manifest_contains(sp) {
                save.request_load(sp);
                // The section's true content is now in flight until the save thread
                // answers (and the overlay applies): the sim guard blocks mutation
                // and the harvest skips persisting it meanwhile.
                self.awaited_overlays.insert(sp);
            }
        }
    }

    /// Install one column's shared gen data: set the per-column biome + an initial
    /// bare-ground surface heightmap (the pre-feature top non-air, authoritative for
    /// skylight/spawn before the surface sections stream in), then keep the `Arc` for
    /// driving per-section jobs.
    fn install_column_gen(&mut self, pos: ChunkPos, col: Arc<ColumnGen>) {
        {
            let column = self.ensure_column(pos);
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    column.set_biome(x, z, col.biome_at(x, z));
                    // Submerged / floorless columns top out at the waterline; land cave
                    // mouths use their post-cave top so skylight can enter shafts.
                    column.set_surface_y(x, z, col.heightmap_surface_y(x, z));
                }
            }
        }
        self.column_gen.insert(pos, col);
    }

    /// Evict everything no longer wanted: columns that left the horizontal radius (whole
    /// column), and sections of kept columns that left the vertical window. Modified /
    /// entity-bearing sections are harvested + persisted first (same gate as autosave).
    fn unload_far(&mut self, target: LoadTarget, vertical_moved: bool) {
        let vwindow = Self::vertical_window(target.center_cy, 2);

        let drop_columns: Vec<ChunkPos> = self
            .columns
            .keys()
            .filter(|p| !Self::column_kept(target, **p))
            .copied()
            .collect();
        let drop_sections: Vec<SectionPos> = if vertical_moved {
            self.sections
                .keys()
                .filter(|sp| {
                    // Cheapest rejection first: almost every section is still inside
                    // the player window, so answer that with two integer compares
                    // before the column-shape test and the per-column surface band.
                    if vwindow.contains(&sp.cy) {
                        return false;
                    }
                    let cp = sp.chunk_pos();
                    Self::column_kept(target, cp)
                        && !self.column_gen.get(&cp).is_some_and(|col| {
                            Self::surface_window_for_column(col, 2).contains(&sp.cy)
                        })
                })
                .copied()
                .collect()
        } else {
            Vec::new()
        };

        // Persist (harvesting entities into the record) before anything leaves memory.
        if self.save.is_some() {
            let mut snaps = Vec::new();
            for &cpos in &drop_columns {
                for cy in Self::column_section_range() {
                    if let Some(snap) =
                        self.harvest_section_snapshot(SectionPos::new(cpos.cx, cy, cpos.cz))
                    {
                        snaps.push(snap);
                    }
                }
            }
            for &sp in &drop_sections {
                if let Some(snap) = self.harvest_section_snapshot(sp) {
                    snaps.push(snap);
                }
            }
            if let Some(save) = self.save.as_mut() {
                save.save_sections(snaps);
            }
        }

        for pos in drop_columns {
            self.remove_column(pos);
            self.drop_overlays_for_column(pos);
        }
        for sp in drop_sections {
            self.remove_section(sp);
            self.pending_overlays.remove(&sp);
            self.pending_sections.remove(&sp);
        }
    }

    /// Drop any buffered disk overlays for a column that is no longer wanted, so a
    /// section whose column was evicted before its overlay could land doesn't linger.
    fn drop_overlays_for_column(&mut self, pos: ChunkPos) {
        self.pending_overlays.retain(|sp, _| sp.chunk_pos() != pos);
    }

    fn within_current_keep_radius(&self, pos: ChunkPos) -> bool {
        let Some(target) = self.last_load_target else {
            return true;
        };
        Self::column_kept(target, pos)
    }

    /// Gate for buffering [`StreamEvent`]s in `poll`. Set each tick from event-bus
    /// listener presence, so with no `section_*` handlers the streamer never
    /// touches the buffer. Turning capture off drops anything already buffered.
    pub fn set_stream_event_capture(&mut self, on: bool) {
        if !on {
            self.stream_events.clear();
        }
        self.stream_events_enabled = on;
    }

    /// Drain the section stream events buffered by `poll` since the last take.
    pub fn take_stream_events(&mut self) -> Vec<StreamEvent> {
        std::mem::take(&mut self.stream_events)
    }

    /// Poll the worker and the save thread, then ingest: install each landed column's
    /// shared data (and kick off its per-section jobs), install generated sections,
    /// overlay any player-modified sections read from disk, and queue the affected
    /// sections for heightmap refresh + light + mesh. Returns the number of columns whose
    /// shared data was installed this call.
    pub fn poll(&mut self) -> usize {
        let target = self
            .last_load_target
            .unwrap_or_else(|| LoadTarget::new(0, 0, 0, self.render_dist));
        let mut new_columns = 0usize;
        let mut new_column_positions: Vec<ChunkPos> = Vec::new();
        let mut ingested: Vec<SectionPos> = Vec::new();

        // 1. Drain worker outputs: column data, then the sections generated from it.
        //    Budgeted so a big burst (e.g. a vertical move that re-streams a whole disc
        //    layer) spreads its main-thread install/mark cost over a few frames instead of
        //    one giant spike; the rest stays buffered in the channel for next poll.
        let drain_start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < GEN_DRAIN_MIN_PER_POLL || drain_start.elapsed() < GEN_DRAIN_TIME_BUDGET {
            let Some(out) = self.worker.try_recv() else {
                break;
            };
            drained += 1;
            match out {
                GenOutput::Column { pos, col } => {
                    let was_pending = self.pending.remove(&pos).is_some();
                    if !was_pending {
                        continue;
                    }
                    if !self.within_current_keep_radius(pos) {
                        continue;
                    }
                    self.install_column_gen(pos, col);
                    new_columns += 1;
                    new_column_positions.push(pos);
                }
                // A panicked gen job: clear the pending flag so the position can be
                // re-requested (or finally judged absent) instead of staying
                // in-flight forever — which would both hide the terrain and freeze
                // the sim guard around it.
                GenOutput::ColumnFailed(pos) => {
                    self.pending.remove(&pos);
                }
                GenOutput::SectionFailed(sp) => {
                    self.pending_sections.remove(&sp);
                }
                GenOutput::Section { sp, section } => {
                    if !self.pending_sections.remove(&sp) {
                        continue;
                    }
                    if !self.within_current_keep_radius(sp.chunk_pos())
                        || !self.column_gen.contains_key(&sp.chunk_pos())
                    {
                        continue;
                    }
                    self.sections.insert(sp, section);
                    self.refresh_block_entity_index(sp);
                    self.refresh_particle_emitter_index(sp);
                    self.classify_deep_on_install(sp);
                    if self.stream_events_enabled {
                        self.stream_events.push(StreamEvent::Generated(sp));
                    }
                    ingested.push(sp);
                }
            }
        }

        // 2. Newly-installed columns: submit their vertical window's section jobs now.
        for pos in new_column_positions {
            self.request_sections_for_column(pos, target);
        }

        // 3. Saved sections read back from disk. Buffer them until their generated section
        //    has landed (disk usually beats noise-gen), then overlay below so the saved
        //    blocks win over the generated base.
        while let Some(loaded) = self.save.as_ref().and_then(|s| s.poll_loaded()) {
            let sp = loaded.pos;
            // The save thread answered: the record is no longer in flight (whatever
            // the answer), so the sim guard must not keep the section blocked.
            self.awaited_overlays.remove(&sp);
            if !self.within_current_keep_radius(sp.chunk_pos()) {
                continue;
            }
            let Some(section) = loaded.section else {
                continue; // missing/corrupt record: generation stands for this section.
            };
            self.pending_overlays
                .insert(sp, (section, loaded.entities, loaded.mobs));
        }

        // 4. Overlay any buffered saved sections whose generated section is now installed.
        let overlaid = self.apply_pending_overlays();
        if self.stream_events_enabled {
            for sp in &overlaid {
                self.stream_events.push(StreamEvent::Loaded(*sp));
            }
        }
        for sp in &overlaid {
            if !ingested.contains(sp) {
                ingested.push(*sp);
            }
        }

        if ingested.is_empty() {
            // Deferred first-time sections can become ready without a fresh ingest
            // (e.g. a target move re-shaped the wanted set), so always re-check.
            self.flush_settled_deferred(target);
            self.request_missing_columns(target);
            return new_columns;
        }

        // 5. Refresh each touched column's heightmap now that more sections exist.
        let mut touched: Vec<ChunkPos> = Vec::new();
        for sp in &ingested {
            let cp = sp.chunk_pos();
            if !touched.contains(&cp) {
                touched.push(cp);
            }
        }
        for cp in touched {
            self.recompute_column_heightmap(cp);
        }

        // 6. Light + remesh the affected sections and their neighbours. Each ingested
        //    section dirties its whole 3×3×3 (border face culling + light sampling), but
        //    those neighbourhoods overlap massively for a contiguous batch — so collect the
        //    UNIQUE affected set once and mark each section a single time, instead of
        //    O(54 × ingested) redundant marks.
        let mut affected: Vec<SectionPos> = Vec::new();
        let mut seen: FxHashSet<SectionPos> = FxHashSet::default();
        for sp in &ingested {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let p = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                        if seen.insert(p) {
                            affected.push(p);
                        }
                    }
                }
            }
        }
        for &sp in &affected {
            // An all-air section (the sky band) emits nothing, so settle it immediately
            // instead of queuing mesh work for it.
            if self.clear_mesh_if_section_produces_no_mesh(sp) {
                continue;
            }
            // A section that already produced output (a baked light cube or an installed
            // mesh) is genuinely stale — relight/remesh it now. One that has produced
            // NOTHING yet is deferred until its generation neighbourhood settles, so its
            // FIRST bake and mesh run exactly once instead of once per landing neighbour
            // (the bulk of streaming's rebake/remesh churn came from this marking).
            let built = self.meshes.contains_key(&sp)
                || self.sections.get(&sp).is_some_and(|s| s.has_baked_light());
            if built {
                self.mark_light_dirty_pos(sp);
                self.queue_dirty_mesh(sp);
            } else if self.sections.contains_key(&sp) {
                // Invalidate (and unqueue) any bake taken from the pre-landing
                // neighbourhood so the settled re-request isn't dedup-dropped.
                self.mark_light_dirty_pos(sp);
                self.light_bakes.cancel(sp);
                self.light_deferred.insert(sp);
            }
        }
        self.flush_settled_deferred(target);

        // 7. Kick generated/overlaid water that now has somewhere to flow.
        self.queue_loaded_section_water_updates(&ingested);
        self.request_missing_columns(target);
        new_columns
    }

    /// Whether everything the FIRST light/mesh of `sp` could read has landed: each
    /// 3×3×3 neighbour is loaded, or is provably not coming under `target` — outside
    /// the wanted shape, deliberately skipped by its landed column (sky / outside the
    /// vertical+surface window), or out of world range — so absent-as-air is its final
    /// state. A neighbour still pending (or whose column is pending or wanted but not
    /// yet landed) means a bake now would just be redone when it arrives.
    fn gen_neighborhood_settled(&self, sp: SectionPos, target: LoadTarget) -> bool {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let n = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    if !SectionPos::cy_in_range(n.cy) || self.sections.contains_key(&n) {
                        continue;
                    }
                    if self.pending_sections.contains(&n) {
                        return false;
                    }
                    let cp = n.chunk_pos();
                    if self.column_gen.contains_key(&cp) {
                        continue;
                    }
                    if self.pending.contains_key(&cp) || Self::column_wanted(target, cp) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Flush deferred first-time sections whose generation neighbourhood has settled:
    /// request the single light bake and queue the single first mesh. Sections whose
    /// saved overlay is still buffered stay parked so the bake reads the saved blocks,
    /// not the generated base it is about to replace.
    fn flush_settled_deferred(&mut self, target: LoadTarget) {
        if self.light_deferred.is_empty() {
            return;
        }
        let ready: Vec<SectionPos> = self
            .light_deferred
            .iter()
            .copied()
            .filter(|sp| {
                !self.pending_overlays.contains_key(sp)
                    && self.gen_neighborhood_settled(*sp, target)
            })
            .collect();
        for sp in ready {
            self.light_deferred.remove(&sp);
            let Some(section) = self.sections.get(&sp) else {
                continue;
            };
            // Fully-opaque sections skip baking on both sides of the mesh pump's
            // light gate (their faces cull against solid cells and never sample light).
            if !section.all_opaque() {
                let key = target.section_priority_key(sp);
                self.light_bakes
                    .request(key, sp, &self.sections, &self.columns);
            }
            self.queue_dirty_mesh(sp);
        }
    }

    /// Whether the live mob list is a complete census of the loaded area's mobs.
    ///
    /// Saved mobs ride in section records and only rejoin the live list when their
    /// record is applied (`apply_pending_overlays`). Until then the list undercounts,
    /// so anything comparing it against a population cap (natural spawning) must hold
    /// off — otherwise every join refills the caps during the streaming window and the
    /// saved mobs then land on top (a per-session population ratchet).
    ///
    /// The census is complete when nothing that can still carry an uncounted saved mob
    /// is outstanding: no saved record awaited from disk or buffered unapplied, and no
    /// column gen in flight (a column's manifest records are only requested once its
    /// gen data installs — see `submit_section_job`). Wanted-but-unsubmitted columns
    /// can't hide here: `request_missing_columns` runs every target update and poll,
    /// so a missing wanted column always shows up in `pending` first.
    pub fn mob_census_settled(&self) -> bool {
        self.pending.is_empty()
            && self.awaited_overlays.is_empty()
            && self.pending_overlays.is_empty()
    }

    /// Overlay every buffered saved section whose generated section is present: replace
    /// it with the saved blocks and restore its drops/mobs. Heightmap refresh is left to
    /// the caller (`poll` recomputes every touched column once). Returns the overlaid
    /// section positions.
    fn apply_pending_overlays(&mut self) -> Vec<SectionPos> {
        let ready: Vec<SectionPos> = self
            .pending_overlays
            .keys()
            .copied()
            .filter(|sp| self.sections.contains_key(sp))
            .collect();
        for sp in &ready {
            let (section, entities, mobs) = self.pending_overlays.remove(sp).unwrap();
            // The record carried drops or mobs: remember that, so a later flush that finds
            // the section free of them rewrites the record instead of leaving stale
            // entities to resurrect (cross-session dupe).
            if !entities.is_empty() || !mobs.is_empty() {
                if let Some(save) = self.save.as_mut() {
                    save.note_record_holds_entities(*sp);
                }
            }
            self.sections.insert(*sp, Arc::new(section));
            self.refresh_block_entity_index(*sp);
            self.refresh_particle_emitter_index(*sp);
            self.dropped_items.extend(entities);
            self.restore_mobs(mobs);
        }
        ready
    }

    /// Kick generated/overlaid source water into flowing once its loaded neighbourhood
    /// gives it somewhere to go: down into air, or sideways into air. Reads neighbours by
    /// world coordinate (so it crosses section and column seams) and only flows into a
    /// neighbour that is actually loaded, so water never spills into a not-yet-streamed
    /// void.
    ///
    /// The kick is also the RE-ARM for simulation work the streaming-finality guard
    /// dropped (`world::sim_guard`): whichever side of a water-air seam lands LAST
    /// re-queues the contact, so no flow is permanently lost to gating. Three scans
    /// per ingested section, each cheap in the bulk cases:
    /// - water + air inside the section: the full interior scan (shores, waterfalls);
    /// - water without air (ocean interior, water over a sealed floor): only the five
    ///   outflow boundary planes, and only against a loaded neighbour that holds air —
    ///   calm open ocean skips every plane by summary;
    /// - any air: the five inflow boundary planes against loaded water-holding
    ///   neighbours, queueing the NEIGHBOUR's water cell — the cross-seam case
    ///   neither section's own water scan can see (its water, this section's air).
    pub(super) fn queue_loaded_section_water_updates(&mut self, ingested: &[SectionPos]) {
        let air = Block::Air.id();
        let water = Block::Water.id();
        let mut updates: Vec<IVec3> = Vec::new();
        for sp in ingested {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            let (ox, oy, oz) = sp.origin_world();
            let has_water = section.has_water();
            let has_air = section.has_air();

            if has_water && has_air {
                let blocks = section.blocks_slice();
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            if blocks[section_idx(lx, ly, lz)] != water {
                                continue;
                            }
                            let wx = ox + lx as i32;
                            let wy = oy + ly as i32;
                            let wz = oz + lz as i32;
                            // Down + the four horizontals (air above is a normal
                            // surface and does not start flow).
                            let neighbors = [
                                (wx, wy - 1, wz),
                                (wx - 1, wy, wz),
                                (wx + 1, wy, wz),
                                (wx, wy, wz - 1),
                                (wx, wy, wz + 1),
                            ];
                            if neighbors.iter().any(|&(nx, ny, nz)| {
                                self.section_loaded_at(nx, ny, nz)
                                    && self.chunk_block(nx, ny, nz) == air
                            }) {
                                updates.push(IVec3::new(wx, wy, wz));
                            }
                        }
                    }
                }
            } else if has_water {
                // No air inside: only boundary water can flow, and only outward
                // through the five outflow faces.
                let blocks = section.blocks_slice();
                for &(dx, dy, dz) in &KICK_OUTFLOW_DIRS {
                    let npos = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    let Some(ns) = self.sections.get(&npos) else {
                        continue; // absent: its own landing kick handles the seam
                    };
                    if !ns.has_air() {
                        continue; // full water/stone plane cannot accept flow
                    }
                    for a in 0..SECTION_SIZE {
                        for b in 0..SECTION_SIZE {
                            let (lx, ly, lz) = boundary_cell(dx, dy, dz, a, b);
                            if blocks[section_idx(lx, ly, lz)] != water {
                                continue;
                            }
                            let (wx, wy, wz) = (
                                ox + lx as i32 + dx,
                                oy + ly as i32 + dy,
                                oz + lz as i32 + dz,
                            );
                            if self.chunk_block(wx, wy, wz) == air {
                                updates.push(IVec3::new(wx - dx, wy - dy, wz - dz));
                            }
                        }
                    }
                }
            }

            if has_air {
                // Water in a LOADED neighbour may now have this section's air to
                // flow into: from above (falls in) or from the four sides.
                let blocks = section.blocks_slice();
                for &(dx, dy, dz) in &KICK_INFLOW_DIRS {
                    let npos = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    let Some(ns) = self.sections.get(&npos) else {
                        continue;
                    };
                    if !ns.has_water() {
                        continue;
                    }
                    for a in 0..SECTION_SIZE {
                        for b in 0..SECTION_SIZE {
                            let (lx, ly, lz) = boundary_cell(dx, dy, dz, a, b);
                            if blocks[section_idx(lx, ly, lz)] != air {
                                continue;
                            }
                            let (nx, ny, nz) = (
                                ox + lx as i32 + dx,
                                oy + ly as i32 + dy,
                                oz + lz as i32 + dz,
                            );
                            if self.chunk_block(nx, ny, nz) == water {
                                updates.push(IVec3::new(nx, ny, nz));
                            }
                        }
                    }
                }
            }
        }
        for pos in updates {
            self.queue_block_update(pos);
        }
    }
}

/// Water can leave a section down or sideways (never up).
const KICK_OUTFLOW_DIRS: [(i32, i32, i32); 5] =
    [(0, -1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];
/// Water can enter a section's air from above (falling) or from the sides
/// (never rising from below).
const KICK_INFLOW_DIRS: [(i32, i32, i32); 5] =
    [(0, 1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)];

/// The section-local cell on the boundary plane facing `(dx,dy,dz)`, indexed by
/// the plane's two free axes `(a, b)`.
#[inline]
fn boundary_cell(dx: i32, dy: i32, dz: i32, a: usize, b: usize) -> (usize, usize, usize) {
    let hi = SECTION_SIZE - 1;
    match (dx, dy, dz) {
        (1, 0, 0) => (hi, a, b),
        (-1, 0, 0) => (0, a, b),
        (0, 1, 0) => (a, hi, b),
        (0, -1, 0) => (a, 0, b),
        (0, 0, 1) => (a, b, hi),
        _ => (a, b, 0),
    }
}

/// Split a whole-column [`Chunk`] (a 0..256 `generate_chunk` output, or a hand-built
/// fixture) into cubic [`Section`]s plus its [`Column`] data, adding solid-stone
/// sections for the range below y=0. All-air sections are skipped (absent reads as
/// air). TEST/FIXTURE helper only: the live streamer generates per section
/// (`ChunkGenerator::generate_section`), never via a 256-tall intermediate. Retained so
/// the many column-era test fixtures (`insert_chunk_for_test`) keep working.
#[cfg(test)]
pub(super) fn split_generated_column(chunk: &Chunk) -> (Column, Vec<(i32, Section)>) {
    let cx = chunk.cx;
    let cz = chunk.cz;
    let mut column = Column::new();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            column.set_biome(x, z, chunk.biome_at(x, z));
            column.set_surface_y(x, z, chunk.surface_y(x, z));
        }
    }

    let mut out: Vec<(i32, Section)> = Vec::new();

    // Surface column: the generator's 0..256 output → sections cy 0..15.
    let surface_sections = (CHUNK_SY / SECTION_SIZE) as i32;
    for cy in 0..surface_sections {
        let mut section = Section::new(cx, cy, cz);
        let mut any = false;
        {
            let dst = section.blocks_slice_mut();
            for ly in 0..SECTION_SIZE {
                let wy = cy as usize * SECTION_SIZE + ly;
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        let id = chunk.block_raw(x, wy, z);
                        if id != 0 {
                            dst[section_idx(x, ly, z)] = id;
                            any = true;
                        }
                    }
                }
            }
        }
        if !any {
            continue; // all-air section: absent reads as air.
        }
        copy_generated_water(chunk, cy, &mut section);
        section.recompute_random_tick_count();
        section.recompute_opaque_count();
        out.push((cy, section));
    }

    // Expanded range below y=0: solid stone, so caves have somewhere to carve.
    for cy in SECTION_MIN_CY..0 {
        let mut section = Section::new(cx, cy, cz);
        {
            let dst = section.blocks_slice_mut();
            for d in dst.iter_mut() {
                *d = Block::Stone.id();
            }
        }
        section.recompute_random_tick_count();
        section.recompute_opaque_count();
        out.push((cy, section));
    }

    (column, out)
}

/// Carry the generated column's water-flow metadata for section `cy` into `section`,
/// so generated rivers/pools keep their source/falloff state through the split.
#[cfg(test)]
fn copy_generated_water(chunk: &Chunk, cy: i32, section: &mut Section) {
    let water = Block::Water.id();
    for ly in 0..SECTION_SIZE {
        let wy = cy as usize * SECTION_SIZE + ly;
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                if chunk.block_raw(x, wy, z) == water {
                    section.set_water(x, ly, z, Block::Water, chunk.water_meta(x, wy, z));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block entity arriving through the saved-section overlay path (not a live
    /// placement) must land in the block-entity index, or it renders/ticks as if
    /// it didn't exist after a reload.
    #[test]
    fn overlaid_saved_section_keeps_its_block_entities_live() {
        let mut world = World::new(0, 4);
        let sp = SectionPos::new(0, 4, 0);
        world.ensure_column(sp.chunk_pos());
        // The generated base the overlay replaces.
        world.sections.insert(sp, Arc::new(Section::new(0, 4, 0)));
        // A saved section carrying a chest lands from disk.
        let mut saved = Section::new(0, 4, 0);
        saved.insert_chest(0, 0, 0, crate::chest::Chest::default());
        world
            .pending_overlays
            .insert(sp, (saved, Vec::new(), Vec::new()));
        world.apply_pending_overlays();

        let mut out = Vec::new();
        world.collect_chests(&mut out);
        assert_eq!(out.len(), 1, "the overlaid chest must be collected");
    }

    /// Natural spawning must not run while saved mob records are still streaming
    /// in: the caps compare against the live list, which undercounts until every
    /// record is applied — spawning through that window duplicates the population
    /// on every world join.
    #[test]
    fn natural_spawning_waits_for_the_saved_mob_census() {
        let mut world = World::new(0, 1);
        let mut chunk = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Grass);
                chunk.set_biome(x, z, crate::biome::Biome::Plains.id());
            }
        }
        world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
        world.last_load_target = Some(LoadTarget::new(0, 4, 0, 1));
        let player = crate::mathh::Vec3::new(1000.0, 1000.0, 1000.0);

        world.awaited_overlays.insert(SectionPos::new(0, 4, 0));
        for _ in 0..200 {
            assert!(
                world.spawn_mobs_tick(player).is_empty(),
                "spawned while a saved mob record was still in flight"
            );
        }

        world.awaited_overlays.clear();
        let spawned: usize = (0..200).map(|_| world.spawn_mobs_tick(player).len()).sum();
        assert!(
            spawned > 0,
            "spawning never resumed after the census settled"
        );
    }

    #[test]
    fn split_keeps_surface_blocks_and_adds_stone_below() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block(1, 64, 2, Block::Stone);
        chunk.set_block(3, 70, 4, Block::Grass);
        let (_column, sections) = split_generated_column(&chunk);

        // Surface block lands in section cy 4 (y 64) at local y 0.
        let s4 = sections.iter().find(|(cy, _)| *cy == 4).expect("cy 4");
        assert_eq!(s4.1.block_raw(1, 0, 2), Block::Stone.id());
        // Below-zero range is solid stone (room for caves).
        let below = sections.iter().find(|(cy, _)| *cy == -1).expect("cy -1");
        assert_eq!(below.1.block_raw(0, 0, 0), Block::Stone.id());
        assert_eq!(below.1.block_raw(8, 8, 8), Block::Stone.id());
    }

    #[test]
    fn generated_water_metadata_survives_the_split() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block(5, 64, 5, Block::Stone);
        chunk.set_water(5, 65, 5, Block::Water, 0x07);
        let (_column, sections) = split_generated_column(&chunk);
        let s4 = sections.iter().find(|(cy, _)| *cy == 4).expect("cy 4");
        assert_eq!(s4.1.block_raw(5, 1, 5), Block::Water.id());
        assert_eq!(s4.1.water_meta(5, 1, 5), 0x07, "falloff metadata carried");
    }

    #[test]
    fn water_kick_queues_source_water_over_a_drop() {
        // A source-water cell with air directly below (and that section loaded) must be
        // kicked into flowing on load. Build the section directly (no set_block_world, so
        // nothing else queues an update) — local y 1 water over local y 0 air.
        let mut world = World::new(0, 0);
        let mut section = Section::new(0, 4, 0);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                section.set_block(x, 0, z, Block::Stone); // world y 64 floor
            }
        }
        section.set_block(4, 0, 4, Block::Air); // carve a hole at world (4,64,4)
        section.set_water(4, 1, 4, Block::Water, 0); // source water at world (4,65,4)
        world.insert_section_for_test(SectionPos::new(0, 4, 0), section);

        world.queue_loaded_section_water_updates(&[SectionPos::new(0, 4, 0)]);
        // The water over the carved hole has a loaded air neighbour below, so the kick
        // queued it: re-queuing the same cell now returns false (already pending).
        assert!(
            !world.queue_block_update(IVec3::new(4, 65, 4)),
            "water over a loaded air drop is kicked into flowing"
        );
        // A different, un-queued cell still returns true — the kick wasn't indiscriminate.
        assert!(world.queue_block_update(IVec3::new(0, 65, 0)));
    }

    #[test]
    fn high_flight_still_wants_the_surface_band() {
        let generator = crate::worldgen::driver::ChunkGenerator::new(0x51EED);
        let col = generator.generate_column_gen(0, 0);
        let cys = World::wanted_section_cys(&col, SECTION_MAX_CY + 100, 0);
        let surface_cy = col
            .surf_range()
            .0
            .max(SEA_LEVEL)
            .div_euclid(SECTION_SIZE as i32);

        assert!(
            cys.contains(&SECTION_MAX_CY),
            "high flight still wants the clamped player/top window"
        );
        assert!(
            cys.contains(&surface_cy),
            "high flight must retain/generate the visible surface band"
        );
    }

    #[test]
    fn facing_streaming_keeps_a_safety_ring_but_skips_far_behind() {
        let target = LoadTarget::new_facing(0, 5, 0, 16, 1.0, 0.0);

        assert!(
            World::column_wanted(target, ChunkPos::new(10, 0)),
            "columns ahead of the camera are wanted"
        );
        assert!(
            !World::column_wanted(target, ChunkPos::new(-10, 0)),
            "far columns behind the camera are not requested"
        );
        assert!(
            World::column_wanted(target, ChunkPos::new(-OMNI_LOAD_RADIUS, 0)),
            "the local safety ring remains omnidirectional"
        );
        assert!(
            !World::column_kept(target, ChunkPos::new(-20, 0)),
            "columns beyond circular unload hysteresis are evicted"
        );
    }

    #[test]
    fn facing_streaming_priority_is_near_first_with_forward_tiebreak() {
        let target = LoadTarget::new_facing(0, 5, 0, 16, 1.0, 0.0);

        assert!(
            target.column_priority_key(ChunkPos::new(0, 2))
                < target.column_priority_key(ChunkPos::new(16, 0)),
            "near terrain must not lose to the far edge just because it is ahead"
        );
        assert!(
            target.column_priority_key(ChunkPos::new(6, 0))
                < target.column_priority_key(ChunkPos::new(0, 6)),
            "outside the safety ring, same-distance work in the forward cone wins"
        );
    }

    #[test]
    fn first_bake_defers_until_generation_neighborhood_settles() {
        use std::sync::Arc;

        let mut world = World::new(0x51EED, 4);
        let target = LoadTarget::new(0, 4, 0, 4);
        world.last_load_target = Some(target);
        let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let cp = ChunkPos::new(dx, dz);
                world
                    .column_gen
                    .insert(cp, Arc::new(generator.generate_column_gen(dx, dz)));
                world.ensure_column(cp);
            }
        }

        // A fresh, never-lit section whose neighbour above is still generating.
        let sp = SectionPos::new(0, 4, 0);
        let mut section = Section::new(0, 4, 0);
        section.set_block(0, 0, 0, Block::Stone);
        world.sections.insert(sp, Arc::new(section));
        let generating = SectionPos::new(0, 5, 0);
        world.pending_sections.insert(generating);
        world.light_deferred.insert(sp);

        world.flush_settled_deferred(target);
        assert!(
            world.light_deferred.contains(&sp),
            "a neighbour's gen is in flight: the first bake must wait"
        );
        assert!(
            !world.light_bakes.has_pending(),
            "no bake may be requested from a half-landed neighbourhood"
        );

        // The neighbour lands (or is discarded): the neighbourhood is now settled —
        // every other absent neighbour belongs to a landed column that skipped it.
        world.pending_sections.remove(&generating);
        world.flush_settled_deferred(target);
        assert!(
            !world.light_deferred.contains(&sp),
            "settled sections leave the deferred set"
        );
        assert!(
            world.light_bakes.has_pending(),
            "the single first bake fires on settle"
        );
        assert!(
            !world.dirty_meshes.is_empty(),
            "the first mesh queues alongside the first bake"
        );
    }

    #[test]
    fn stale_pending_columns_are_pruned_to_current_view_shape() {
        let mut world = World::new(0, 16);
        let old_front = ChunkPos::new(10, 0);
        let safety_ring = ChunkPos::new(OMNI_LOAD_RADIUS, 0);
        world.pending.insert(old_front, ());
        world.pending.insert(safety_ring, ());

        let target = LoadTarget::new_facing(0, 5, 0, 16, -1.0, 0.0);
        world.prune_stale_column_requests(target);

        assert!(
            !world.pending.contains_key(&old_front),
            "queued work outside the new forward/safety shape should be dropped"
        );
        assert!(
            world.pending.contains_key(&safety_ring),
            "the omnidirectional safety ring stays queued"
        );
    }

    #[test]
    fn view_turn_requests_sections_for_newly_wanted_loaded_columns() {
        use std::sync::Arc;

        let mut world = World::new(0x51EED, 8);
        let old = LoadTarget::new_facing(0, 5, 0, 8, 1.0, 0.0);
        let newly_front = ChunkPos::new(-8, 0);
        assert!(
            !World::column_wanted(old, newly_front),
            "test setup: column starts outside the old forward sector"
        );

        let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
        let col = Arc::new(generator.generate_column_gen(newly_front.cx, newly_front.cz));
        world.column_gen.insert(newly_front, col);
        world.last_load_target = Some(old);

        world.update_load_facing(0, 5, 0, -1.0, 0.0);

        assert!(
            world
                .pending_sections
                .iter()
                .any(|sp| sp.chunk_pos() == newly_front),
            "a generated column that enters the view cone must request its sections"
        );
    }

    /// The whole cubic pipeline in one go (worldgen-tests only — it runs the real gen +
    /// save threads): a column streams in and meshes, a block edited into the open air
    /// above the surface materializes its section, and after a flush + evict + reload the
    /// edit comes back via the disk overlay. Generate → mesh → edit → save → reload.
    #[cfg(feature = "worldgen-tests")]
    #[test]
    fn cubic_world_generates_meshes_saves_and_reloads_an_edit() {
        use std::time::Duration;

        let dir = std::env::temp_dir().join(format!("llamacraft-cubic-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let opened = crate::save::open_at(dir.clone()).expect("open save");
        let mut world = World::new(0x51EED, 2);
        world.attach_save(opened.save);

        // Stream the origin column: generate (worker) + ingest. The later edit lands well
        // above the active vertical window; reload coverage comes from the save manifest.
        world.update_load(0, 8, 0);
        let mut spun = 0;
        while !world.chunk_loaded(0, 0) && spun < 3000 {
            world.poll();
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert!(world.chunk_loaded(0, 0), "the origin column streamed in");

        // Mesh the loaded sections. Poll + sleep between budgets so the async light bakes
        // the mesher waits on can finish, exactly as they do between real frames (a tight
        // no-delay loop never lets the light pool produce a result).
        for _ in 0..400 {
            world.poll();
            world.tick_mesh_budget(64);
            if world.iter_meshes().next().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            world.iter_meshes().next().is_some(),
            "at least one section meshed"
        );

        // Edit a block into the open air well above any terrain (max surface ~171): this
        // materializes section (0,15,0) on write.
        let edit = IVec3::new(4, 250, 4);
        assert!(world.set_block_world(edit.x, edit.y, edit.z, Block::Stone));
        assert_eq!(world.chunk_block(edit.x, edit.y, edit.z), Block::Stone.id());

        // Flush to disk, then wait for the save thread to drain by reading the section back
        // through a blocking load (the channel is ordered, so this trails the write).
        world.flush_modified_chunks();
        let sp = SectionPos::from_world(edit.x, edit.y, edit.z).unwrap();
        {
            let save = world.save().expect("save attached");
            assert!(
                save.manifest_contains(sp),
                "edit's section is in the manifest"
            );
            save.request_load(sp);
            let mut got = None;
            for _ in 0..1500 {
                if let Some(l) = save.poll_loaded() {
                    got = Some(l);
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let loaded = got.expect("section read back from disk");
            let section = loaded.section.expect("section record decodes");
            assert_eq!(
                section.block_raw(4, 250usize.rem_euclid(16), 4),
                Block::Stone.id(),
                "the edit persisted to disk"
            );
        }

        // Evict everything, then re-stream: gen rebuilds the column and the saved section
        // overlays the edit back on.
        world.clear_world();
        world.last_load_target = None;
        world.update_load(0, 8, 0);
        let mut spun = 0;
        while world.chunk_block(edit.x, edit.y, edit.z) != Block::Stone.id() && spun < 3000 {
            world.poll();
            std::thread::sleep(Duration::from_millis(2));
            spun += 1;
        }
        assert_eq!(
            world.chunk_block(edit.x, edit.y, edit.z),
            Block::Stone.id(),
            "the saved edit overlaid back on after reload"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The defining S3 behaviour: worldgen runs per section, CLOSEST TO THE PLAYER. A
    /// player at the surface streams the surface band but NOT the deep sections below
    /// y=0 (the cave space); descending streams those deep sections in. Proves the
    /// vertical window genuinely bounds generation in 3D rather than batching whole
    /// 256-tall columns.
    #[cfg(feature = "worldgen-tests")]
    #[test]
    fn vertical_window_generates_near_the_player_not_the_whole_column() {
        use std::time::{Duration, Instant};

        let mut world = World::new(0xC0FFEE, 1);
        // y=-60 is deep section cy=-4 (the would-be cave space); y=96 is the surface band.
        let deep = (0, -60, 0);
        let surface = (0, 96, 0);

        // Player near the surface (section cy 6): stream until a surface section lands.
        world.update_load(0, 6, 0);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !world.chunk_loaded(0, 0) && Instant::now() < deadline {
            world.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        // Drain a few more polls so the whole window has a chance to stream in.
        for _ in 0..32 {
            world.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            world.section_loaded_at(surface.0, surface.1, surface.2),
            "a surface section streamed in around the player"
        );
        assert!(
            !world.section_loaded_at(deep.0, deep.1, deep.2),
            "the deep cave-space section is NOT generated while the player is at the surface"
        );

        // Descend to that deep section (cy -4): now it must stream in.
        world.update_load(0, -4, 0);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !world.section_loaded_at(deep.0, deep.1, deep.2) && Instant::now() < deadline {
            world.poll();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            world.section_loaded_at(deep.0, deep.1, deep.2),
            "the deep section streamed in once the player descended to it"
        );
    }
}
