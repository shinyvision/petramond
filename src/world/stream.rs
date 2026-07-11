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

use super::store::{LoadAnchor, LoadTarget, World, WorldRole, VERTICAL_LOAD_RADIUS};

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
const GEN_DRAIN_MIN_PER_POLL: usize = 16;
const GEN_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(750);
const DISK_DRAIN_MIN_PER_POLL: usize = 16;
const DISK_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(750);

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
        debug_assert!(
            self.role != WorldRole::ClientReplica,
            "a replica never generates: sections arrive from the connection"
        );
        let target = LoadTarget::new(cam_chunk_x, cam_chunk_y, cam_chunk_z, self.render_dist);
        self.update_load_target(target);
    }

    fn update_load_target(&mut self, target: LoadTarget) {
        // The single-anchor path: any multi-anchor residue is gone.
        self.extra_load_targets.clear();
        if self.last_load_target == Some(target) {
            if !self.missing_columns_settled {
                self.request_missing_columns(target);
            }
            return;
        }
        let prev = self.last_load_target;
        self.last_load_target = Some(target);
        self.missing_columns_settled = false;
        self.deferred_recheck_needed = true;
        // The player ring and disc edge moved; deep-visibility must re-evaluate.
        self.vis_dirty = true;
        let vertical_moved = prev.is_none_or(|p| p.center_cy != target.center_cy);
        let horizontal_keep_changed =
            prev.is_none_or(|p| p.center != target.center || p.render_dist != target.render_dist);

        self.prune_stale_column_requests(target);
        self.request_missing_columns(target);
        // `request_wanted_sections` re-scans EVERY loaded column's whole vertical window.
        // That full scan only changes existing wanted columns when the vertical centre
        // moves. Horizontal changes can still make an already-generated column newly
        // wanted, so scan only that entering subset instead of every column in the disc.
        if vertical_moved {
            match prev {
                Some(p) => self.request_vertical_delta_sections(p, target),
                None => self.request_wanted_sections(target),
            }
        }
        if !vertical_moved {
            if let Some(prev) = prev {
                if prev.center != target.center || prev.render_dist != target.render_dist {
                    self.request_newly_wanted_sections(prev, target);
                }
            }
        }
        if horizontal_keep_changed || vertical_moved {
            self.unload_far(target, vertical_moved);
        }
    }

    /// N-anchor streaming for the multi-player server: request everything any
    /// anchor wants (submission priority = the MIN key over the anchors, so a
    /// column between two players resolves for whichever is nearer) and keep
    /// everything inside ANY anchor's keep shape. One anchor is exactly
    /// [`update_load`] including its incremental rescan optimizations; the N ≥ 2 path
    /// trades those delta scans for a plain full scan on anchor-set change (bounded by
    /// the anchors' discs, and it runs only on change).
    pub fn update_load_multi(&mut self, anchors: &[LoadAnchor]) {
        debug_assert!(
            self.role != WorldRole::ClientReplica,
            "a replica never generates: sections arrive from the connection"
        );
        // Each anchor streams at ITS connection's radius (view distance),
        // never wider than this world's own `render_dist` budget.
        let radius = |a: &LoadAnchor| a.radius.clamp(1, self.render_dist);
        match anchors {
            [] => {}
            [a] => {
                let target = LoadTarget::new(a.cx, a.cy, a.cz, radius(a));
                self.update_load_target(target);
            }
            _ => {
                let targets: Vec<LoadTarget> = anchors
                    .iter()
                    .map(|a| LoadTarget::new(a.cx, a.cy, a.cz, radius(a)))
                    .collect();
                self.update_load_multi_targets(targets);
            }
        }
    }

    fn update_load_multi_targets(&mut self, targets: Vec<LoadTarget>) {
        let unchanged =
            self.last_load_target == Some(targets[0]) && self.extra_load_targets == targets[1..];
        if unchanged {
            if !self.missing_columns_settled {
                self.request_missing_columns_multi(&targets);
            }
            return;
        }
        self.last_load_target = Some(targets[0]);
        self.extra_load_targets = targets[1..].to_vec();
        self.missing_columns_settled = false;
        self.deferred_recheck_needed = true;
        self.vis_dirty = true;
        self.pending.retain(|pos, job| {
            let keep = targets.iter().any(|t| Self::column_wanted(*t, *pos));
            if !keep {
                if let Some(job) = job {
                    job.cancel();
                }
            }
            keep
        });
        self.request_missing_columns_multi(&targets);
        self.request_wanted_sections_multi(&targets);
        self.unload_far_multi(&targets);
    }

    /// Whether `cp` is wanted under ANY current anchor. The multi-anchor form
    /// of `last_load_target + column_wanted`, used wherever "is this column
    /// coming?" must hold for every player (the sim guard's in-flight
    /// classification, keep checks). Identical to the single check while
    /// `extra_load_targets` is empty.
    pub(super) fn column_wanted_by_any_target(&self, cp: ChunkPos) -> bool {
        self.last_load_target
            .is_some_and(|t| Self::column_wanted(t, cp))
            || self
                .extra_load_targets
                .iter()
                .any(|t| Self::column_wanted(*t, cp))
    }

    fn multi_column_key(targets: &[LoadTarget], pos: ChunkPos) -> i64 {
        targets
            .iter()
            .map(|t| t.column_priority_key(pos))
            .min()
            .expect("at least one target")
    }

    fn multi_section_key(targets: &[LoadTarget], sp: SectionPos) -> i64 {
        targets
            .iter()
            .map(|t| t.section_priority_key(sp))
            .min()
            .expect("at least one target")
    }

    /// [`request_missing_columns`] over the union of the anchors' discs.
    fn request_missing_columns_multi(&mut self, targets: &[LoadTarget]) {
        let submit_limit = MAX_COLUMN_GEN_SUBMITS_PER_TARGET
            .min(MAX_PENDING_COLUMN_GEN_JOBS.saturating_sub(self.pending.len()));
        if submit_limit == 0 {
            return;
        }
        let mut missing: Vec<(i64, ChunkPos)> = Vec::new();
        let mut seen: FxHashSet<ChunkPos> = FxHashSet::default();
        for scan in targets {
            let r = scan.render_dist;
            for dz in -r..=r {
                for dx in -r..=r {
                    let pos = ChunkPos::new(scan.center.cx + dx, scan.center.cz + dz);
                    if !seen.insert(pos) {
                        continue;
                    }
                    if !targets.iter().any(|t| Self::column_wanted(*t, pos)) {
                        continue;
                    }
                    if self.column_gen.contains_key(&pos) || self.pending.contains_key(&pos) {
                        continue;
                    }
                    missing.push((Self::multi_column_key(targets, pos), pos));
                }
            }
        }
        // Same settled short-circuit as the single-anchor scan.
        self.missing_columns_settled = missing.len() <= submit_limit;
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.submit_column_job(priority, pos);
        }
    }

    /// [`request_wanted_sections`] with each column's wanted window = the
    /// union over the anchors that want it.
    fn request_wanted_sections_multi(&mut self, targets: &[LoadTarget]) {
        let mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)> = Vec::new();
        let mut cys: Vec<i32> = Vec::new();
        for (pos, col) in &self.column_gen {
            cys.clear();
            for t in targets {
                if !Self::column_wanted(*t, *pos) {
                    continue;
                }
                for cy in self.wanted_section_cys_for_column(*pos, col, t.center_cy, 0) {
                    if !cys.contains(&cy) {
                        cys.push(cy);
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
                wanted.push((Self::multi_section_key(targets, sp), sp, col.clone()));
            }
        }
        wanted.sort_by_key(|(priority, _, _)| *priority);
        for (priority, sp, col) in wanted {
            self.submit_section_job(priority, sp, col);
        }
    }

    /// [`unload_far`] keeping the UNION of the anchors' keep shapes: a column
    /// (or kept column's section) survives if any anchor still wants it, with
    /// the same hysteresis slack as the single-anchor path.
    fn unload_far_multi(&mut self, targets: &[LoadTarget]) {
        let drop_columns: Vec<ChunkPos> = self
            .columns
            .keys()
            .filter(|p| !targets.iter().any(|t| Self::column_kept(*t, **p)))
            .copied()
            .collect();
        let drop_sections: Vec<SectionPos> =
            self.sections
                .keys()
                .filter(|sp| {
                    if targets
                        .iter()
                        .any(|t| Self::vertical_window(t.center_cy, 2).contains(&sp.cy))
                    {
                        return false;
                    }
                    let cp = sp.chunk_pos();
                    targets.iter().any(|t| Self::column_kept(*t, cp))
                        && !self.column_gen.get(&cp).is_some_and(|col| {
                            Self::surface_window_for_column(col, 2).contains(&sp.cy)
                        })
                })
                .copied()
                .collect();
        self.evict_columns_and_sections(drop_columns, drop_sections);
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

    fn column_in_shape(target: LoadTarget, pos: ChunkPos, slack: i32) -> bool {
        let (dx, dz, r) = Self::column_shape_key(target, pos);
        let radius = (r + slack).max(0);
        dx * dx + dz * dz <= radius * radius
    }

    /// `pub(super)` for the sim guard: an absent column that is wanted under the
    /// current target counts as in-flight, not as never-coming.
    pub(super) fn column_wanted(target: LoadTarget, pos: ChunkPos) -> bool {
        Self::column_in_shape(target, pos, 0)
    }

    /// `pub(super)` for the per-connection terrain sender: its client-side
    /// unload mirrors the streamer's own keep hysteresis.
    pub(super) fn column_kept(target: LoadTarget, pos: ChunkPos) -> bool {
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
        // Everything wanted is loaded or queued after this pass: the per-pump
        // rescan is pure waste until an eviction / failure / anchor change
        // un-settles it (see `missing_columns_settled`). Only valid while this
        // single target IS the whole anchor set — under multi-anchor streaming
        // this scan cannot see the extra anchors' columns, so it must not mark
        // the wider wanted-set settled.
        if self.extra_load_targets.is_empty() {
            self.missing_columns_settled = missing.len() <= submit_limit;
        }
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.submit_column_job(priority, pos);
        }
    }

    /// Queue one column's gen job (or its column-gen cache read) and mark it
    /// pending. Shared by the single- and multi-anchor request scans.
    fn submit_column_job(&mut self, priority: i64, pos: ChunkPos) {
        // "Optimize explored terrain": an explored column's 2D gen data
        // loads from the column-gen cache instead of running the heavy
        // noise job; a decode miss falls back to the worker (poll's cache
        // drain resubmits). Both answers resolve the same `pending` entry.
        let cached = self.optimize_explored_terrain
            && self
                .save
                .as_ref()
                .is_some_and(|s| s.colgen_manifest_contains(pos));
        let job = if cached {
            if let Some(save) = self.save.as_ref() {
                save.request_column_gen(pos, self.seed);
            }
            None
        } else {
            Some(self.worker.submit(
                priority,
                GenJob::Column {
                    pos,
                    seed: self.seed,
                },
            ))
        };
        self.pending.insert(pos, job);
    }

    fn prune_stale_column_requests(&mut self, target: LoadTarget) {
        self.pending.retain(|pos, job| {
            let keep = Self::column_wanted(target, *pos);
            if !keep {
                if let Some(job) = job {
                    job.cancel();
                }
            }
            keep
        });
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
            && !self
                .save
                .as_ref()
                .is_some_and(|s| s.authoritative_manifest_contains(sp))
    }

    /// Queue one section's gen job and, paired with it, ask the save thread for that
    /// section's saved (player-modified) record if one exists — so the disk overlay
    /// lands after the generated base and wins (`apply_pending_overlays`).
    ///
    /// With "Optimize explored terrain" on, a section that exists on disk skips the
    /// gen job entirely: its record is a full section that would have replaced the
    /// generated base anyway, so it installs as the PRIMARY content when the save
    /// thread answers (`poll`), and generation runs only as the corrupt-record
    /// fallback.
    fn submit_section_job(&mut self, key: i64, sp: SectionPos, col: Arc<ColumnGen>) {
        let disk_primary = self.optimize_explored_terrain && self.saved_section_contains(sp);
        if disk_primary {
            self.pending_sections.insert(sp);
            self.disk_primary_sections.insert(sp);
            // The section's true content is in flight until the save thread
            // answers: the sim guard blocks mutation and the harvest skips
            // persisting it meanwhile (same contract as the overlay path).
            self.awaited_overlays.insert(sp);
            if let Some(save) = self.save.as_ref() {
                save.request_load(sp, true);
            }
            return;
        }
        let job = self.worker.submit(
            key,
            GenJob::Section {
                sp,
                col,
                seed: self.seed,
            },
        );
        self.pending_sections.insert(sp);
        self.pending_section_jobs.insert(sp, job);
        if let Some(save) = self.save.as_ref() {
            if save.authoritative_manifest_contains(sp) {
                save.request_load(sp, false);
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
        if self.optimize_explored_terrain
            && self
                .save
                .as_ref()
                .is_some_and(|s| !s.colgen_manifest_contains(pos))
        {
            self.pending_colgen_records
                .push(col.cache_record(self.seed));
        }
        self.column_gen.insert(pos, col);
        self.bump_column_payload_revision(pos);
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
        self.evict_columns_and_sections(drop_columns, drop_sections);
    }

    /// The persist-then-drop tail of unloading: harvest entities + persist
    /// modified sections (same gate as autosave), then evict. Shared by the
    /// single- and multi-anchor unload selections.
    fn evict_columns_and_sections(
        &mut self,
        drop_columns: Vec<ChunkPos>,
        drop_sections: Vec<SectionPos>,
    ) {
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
            self.flush_pending_colgen_records();
        }

        let dropped_any = !drop_columns.is_empty() || !drop_sections.is_empty();
        for pos in drop_columns {
            self.remove_column(pos);
            self.drop_overlays_for_column(pos);
        }
        for sp in drop_sections {
            self.remove_section(sp);
            self.pending_overlays.remove(&sp);
            self.pending_sections.remove(&sp);
            if let Some(job) = self.pending_section_jobs.remove(&sp) {
                job.cancel();
            }
        }
        if dropped_any {
            self.bump_terrain_revision();
        }
    }

    /// Drop any buffered disk overlays for a column that is no longer wanted, so a
    /// section whose column was evicted before its overlay could land doesn't linger.
    fn drop_overlays_for_column(&mut self, pos: ChunkPos) {
        self.pending_overlays.retain(|sp, _| sp.chunk_pos() != pos);
    }

    /// Swap `pos`'s retained `ColumnGen` for its slimmed clone once the column has
    /// no in-flight section gen. In-flight jobs keep their full `Arc`; premature
    /// slimming is safe (a later tree-band job rebuilds the windows), so this is a
    /// memory policy, not a correctness gate.
    ///
    fn slim_settled_column_gen(&mut self, pos: ChunkPos) {
        let Some(col) = self.column_gen.get(&pos) else {
            return;
        };
        if !col.has_feature_windows() {
            return;
        }
        let slim = std::sync::Arc::new(col.slimmed());
        self.column_gen.insert(pos, slim);
    }

    fn within_current_keep_radius(&self, pos: ChunkPos) -> bool {
        let Some(target) = self.last_load_target else {
            return true;
        };
        Self::column_kept(target, pos)
            || self
                .extra_load_targets
                .iter()
                .any(|t| Self::column_kept(*t, pos))
    }

    /// The anchor that wants column `pos` most (min priority key) — the target
    /// a landed column's section window should be built around, so a column
    /// streamed for a far player fills near THAT player's `cy`. The primary
    /// target while single-anchor.
    fn best_target_for_column(&self, target: LoadTarget, pos: ChunkPos) -> LoadTarget {
        let mut best = target;
        let mut best_key = target.column_priority_key(pos);
        for t in &self.extra_load_targets {
            let key = t.column_priority_key(pos);
            if key < best_key {
                best = *t;
                best_key = key;
            }
        }
        best
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
        // Any change to what is loaded / stream-final re-keys the per-connection
        // terrain senders (their wanted-vs-sent rescan gates on this).
        let before = self.stream_finality_fingerprint();
        let new_columns = self.poll_inner();
        if self.stream_finality_fingerprint() != before {
            self.bump_terrain_revision();
        }
        new_columns
    }

    /// The cheap "did loaded/in-flight content change?" probe `poll` brackets
    /// itself with. Within one poll, installs only grow `sections` and every
    /// finality transition shrinks an in-flight set, so length deltas suffice.
    fn stream_finality_fingerprint(&self) -> (usize, usize, usize, usize) {
        (
            self.sections.len(),
            self.pending_sections.len(),
            self.awaited_overlays.len(),
            self.pending_overlays.len(),
        )
    }

    fn poll_inner(&mut self) -> usize {
        debug_assert!(
            self.role != WorldRole::ClientReplica,
            "a replica has no gen/save workers to poll; installs come from the connection"
        );
        let target = self
            .last_load_target
            .unwrap_or_else(|| LoadTarget::new(0, 0, 0, self.render_dist));
        let mut new_columns = 0usize;
        let mut new_column_positions: Vec<ChunkPos> = Vec::new();
        let mut ingested: Vec<SectionPos> = Vec::new();
        let mut ingested_set: FxHashSet<SectionPos> = FxHashSet::default();
        let mut heightmap_recompute: FxHashSet<ChunkPos> = FxHashSet::default();
        // The freshly GENERATED subset of `ingested` (disk loads are tracked
        // separately) — these invalidate their neighbourhood's light in step 6.
        let mut gen_ingested: FxHashSet<SectionPos> = FxHashSet::default();

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
                    // No longer pending and not installed: the column is
                    // missing again — let the scan re-find it.
                    self.missing_columns_settled = false;
                    self.deferred_recheck_needed = true;
                }
                GenOutput::SectionFailed(sp) => {
                    self.pending_sections.remove(&sp);
                    self.pending_section_jobs.remove(&sp);
                    self.queue_deferred_rechecks_around(sp);
                }
                GenOutput::Section { sp, section } => {
                    if !self.pending_sections.remove(&sp) {
                        continue;
                    }
                    self.pending_section_jobs.remove(&sp);
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
                    if ingested_set.insert(sp) {
                        ingested.push(sp);
                    }
                    gen_ingested.insert(sp);
                }
            }
        }

        // 1b. Column-gen cache answers ("Optimize explored terrain"): a hit
        //     installs exactly like a generated column; a miss (corrupt record,
        //     seed/version drift) hands the column to the worker — `pending`
        //     stays set so the existing `GenOutput::Column` arm resolves it.
        let colgen_start = std::time::Instant::now();
        let mut colgen_drained = 0usize;
        while colgen_drained < DISK_DRAIN_MIN_PER_POLL
            || colgen_start.elapsed() < DISK_DRAIN_TIME_BUDGET
        {
            let Some(loaded) = self.save.as_ref().and_then(|s| s.poll_loaded_column_gen()) else {
                break;
            };
            colgen_drained += 1;
            let pos = loaded.pos;
            if !self.pending.contains_key(&pos) {
                continue;
            }
            match loaded.record {
                Some(rec) => {
                    self.pending.remove(&pos);
                    if !self.within_current_keep_radius(pos) {
                        continue;
                    }
                    let col = Arc::new(ColumnGen::from_cache_record(rec));
                    self.install_column_gen(pos, col);
                    new_columns += 1;
                    new_column_positions.push(pos);
                }
                None => {
                    if let Some(save) = self.save.as_mut() {
                        save.note_colgen_load_miss(pos);
                    }
                    let job = self.worker.submit(
                        target.column_priority_key(pos),
                        GenJob::Column {
                            pos,
                            seed: self.seed,
                        },
                    );
                    if let Some(slot) = self.pending.get_mut(&pos) {
                        *slot = Some(job);
                    }
                }
            }
        }

        // 2. Newly-installed columns: submit their vertical window's section jobs now,
        //    around whichever anchor wants each column most.
        for pos in new_column_positions {
            let best = self.best_target_for_column(target, pos);
            self.request_sections_for_column(pos, best);
            self.queue_deferred_rechecks_around_column(pos);
        }

        // 3. Saved sections read back from disk. Disk-primary records ("Optimize
        //    explored terrain" — no gen job was submitted) install immediately;
        //    overlay records buffer until their generated section has landed
        //    (disk usually beats noise-gen), then apply below so the saved
        //    blocks win over the generated base.
        let disk_start = std::time::Instant::now();
        let mut disk_drained = 0usize;
        while disk_drained < DISK_DRAIN_MIN_PER_POLL
            || disk_start.elapsed() < DISK_DRAIN_TIME_BUDGET
        {
            let Some(loaded) = self.save.as_ref().and_then(|s| s.poll_loaded()) else {
                break;
            };
            disk_drained += 1;
            let sp = loaded.pos;
            let loaded_store = loaded.store;
            // The save thread answered: the record is no longer in flight (whatever
            // the answer), so the sim guard must not keep the section blocked.
            self.awaited_overlays.remove(&sp);
            let disk_primary = self.disk_primary_sections.remove(&sp);
            if disk_primary {
                self.pending_sections.remove(&sp);
                self.pending_section_jobs.remove(&sp);
            }
            if !self.within_current_keep_radius(sp.chunk_pos()) {
                continue;
            }
            let Some(section) = loaded.section else {
                if let Some(save) = self.save.as_mut() {
                    save.note_section_load_miss(sp, loaded.store);
                }
                // Missing/corrupt record. Overlay path: generation stands.
                // Disk-primary path: no base exists — generate it after all.
                if disk_primary {
                    if let Some(col) = self.column_gen.get(&sp.chunk_pos()).cloned() {
                        let job = self.worker.submit(
                            target.section_priority_key(sp),
                            GenJob::Section {
                                sp,
                                col,
                                seed: self.seed,
                            },
                        );
                        self.pending_sections.insert(sp);
                        self.pending_section_jobs.insert(sp, job);
                    }
                }
                continue;
            };
            if disk_primary {
                if !self.column_gen.contains_key(&sp.chunk_pos()) {
                    continue; // column evicted while the read was in flight
                }
                if !loaded.entities.is_empty() || !loaded.mobs.is_empty() {
                    if let Some(save) = self.save.as_mut() {
                        save.note_record_holds_entities(sp);
                    }
                }
                self.sections.insert(sp, Arc::new(section));
                self.refresh_block_entity_index(sp);
                self.refresh_particle_emitter_index(sp);
                self.classify_deep_on_install(sp);
                self.dropped_items.extend(loaded.entities);
                self.restore_mobs(loaded.mobs);
                if self.stream_events_enabled {
                    self.stream_events.push(StreamEvent::Loaded(sp));
                }
                if ingested_set.insert(sp) {
                    ingested.push(sp);
                }
                if loaded_store == crate::save::SectionStore::Authoritative {
                    heightmap_recompute.insert(sp.chunk_pos());
                }
            } else {
                self.pending_overlays
                    .insert(sp, (section, loaded.entities, loaded.mobs));
            }
        }

        // 4. Overlay any buffered saved sections whose generated section is now installed.
        let overlaid = self.apply_pending_overlays();
        if self.stream_events_enabled {
            for sp in &overlaid {
                self.stream_events.push(StreamEvent::Loaded(*sp));
            }
        }
        for sp in &overlaid {
            if ingested_set.insert(*sp) {
                ingested.push(*sp);
            }
            heightmap_recompute.insert(sp.chunk_pos());
        }

        // Columns whose burst just finished (a section landed — generated,
        // disk-primary, or overlaid — and nothing is pending for the column any
        // more) retain only the slimmed ColumnGen: the ~15 KB tree windows are
        // dead weight post-gen, and a rare late tree-band job rebuilds them
        // locally (see worldgen::driver::FeatureWindows). This is also the
        // column-gen cache capture point.
        let ingested_columns: FxHashSet<ChunkPos> =
            ingested.iter().map(|sp| sp.chunk_pos()).collect();
        let pending_columns: FxHashSet<ChunkPos> = self
            .pending_sections
            .iter()
            .map(|sp| sp.chunk_pos())
            .collect();
        for pos in &ingested_columns {
            if !pending_columns.contains(pos) {
                self.slim_settled_column_gen(*pos);
            }
        }
        // Bound the column-cache buffer during long exploration flights between
        // autosaves/unloads (each flush is one batched region write).
        if self.pending_colgen_records.len() >= 128 {
            self.flush_pending_colgen_records();
        }

        if ingested.is_empty() {
            // Deferred first-time sections can become ready without a fresh ingest
            // (e.g. a target move re-shaped the wanted set), so always re-check.
            self.flush_settled_deferred_if_needed(target);
            // Belt-and-suspenders single-anchor rescan; skipped once settled
            // (the per-pump anchor update rescans whenever unsettled) and under
            // multi-anchor streaming, where `update_load_multi` owns the scan.
            if !self.missing_columns_settled && self.extra_load_targets.is_empty() {
                self.request_missing_columns(target);
            }
            return new_columns;
        }

        // 5. Derived sections can only raise the analytical bare surface with
        // generated features. Authoritative records may have removed it, so only
        // those columns pay for a full vertical rescan.
        for &sp in &ingested {
            if !heightmap_recompute.contains(&sp.chunk_pos()) {
                self.raise_column_heightmap_from_section(sp);
            }
        }
        for cp in heightmap_recompute {
            self.recompute_column_heightmap(cp);
        }

        // 6. Light + remesh the affected sections and their neighbours. Each ingested
        //    section dirties its whole 3×3×3 (border face culling + light sampling), but
        //    those neighbourhoods overlap massively for a contiguous batch — so collect the
        //    UNIQUE affected set once and mark each section a single time, instead of
        //    O(54 × ingested) redundant marks.
        //
        //    LIGHT INVALIDATION is keyed on how each section landed, by SOURCE.
        //    Fresh GENERATION invalidates its 3×3×3: no persisted bake ever saw
        //    this content. A saved OVERLAY invalidates too: its generated base
        //    was transiently visible, and an in-flight bake may have read the
        //    base. DISK-PRIMARY loads invalidate NOTHING — lit or not (fully
        //    opaque records persist lightless by design), their content is
        //    byte-exactly what every neighbouring record's persisted bake read
        //    (records only persist with settled light) — this is what makes
        //    persisted light load bake-free.
        let overlaid_set: FxHashSet<SectionPos> = overlaid.iter().copied().collect();
        let mut affected: Vec<SectionPos> = Vec::new();
        let mut seen: FxHashSet<SectionPos> = FxHashSet::default();
        let mut light_stale: FxHashSet<SectionPos> = FxHashSet::default();
        for sp in &ingested {
            let invalidates = overlaid_set.contains(sp) || gen_ingested.contains(sp);
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        let p = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                        if seen.insert(p) {
                            affected.push(p);
                        }
                        if invalidates {
                            light_stale.insert(p);
                        }
                    }
                }
            }
        }
        for &sp in &affected {
            // An all-air section (the sky band) emits nothing — settle its MESH
            // immediately. Its light still bakes below (cave pockets must read
            // dark, and headless/replica worlds have no lazy mesh-gate bake).
            let no_mesh_output = self.clear_mesh_if_section_produces_no_mesh(sp);
            let stale = light_stale.contains(&sp);
            if self.meshes.contains_key(&sp) {
                // Already produced a mesh: remesh now (border culling moved).
                // Relight only if a landing actually invalidated it — plus the
                // in-flight-bake race: a bake requested before this landing
                // read the pre-landing neighbourhood, so a still-dirty section
                // re-marks (revision bump) to discard that result.
                if stale || self.sections.get(&sp).is_some_and(|s| s.light_dirty) {
                    self.mark_light_dirty_pos(sp);
                    // The bump invalidated any in-flight bake; unqueue it so the
                    // remesh's re-request isn't dedup-dropped (mirrors the
                    // no-mesh branch below).
                    self.light_bakes.cancel(sp);
                }
                self.queue_dirty_mesh(sp);
            } else if self.sections.contains_key(&sp) {
                // No output yet: its FIRST bake (if its light is dirty at all)
                // and FIRST mesh run once, when the neighbourhood settles —
                // not once per landing neighbour (the bulk of streaming's
                // rebake/remesh churn came from eager marking here).
                if stale {
                    self.mark_light_dirty_pos(sp);
                }
                // Unqueue any bake taken from the pre-landing neighbourhood so
                // the settled re-request isn't dedup-dropped.
                self.light_bakes.cancel(sp);
                let needs_bake = self
                    .sections
                    .get(&sp)
                    .is_some_and(|s| s.light_dirty && !s.all_opaque());
                if !no_mesh_output || needs_bake {
                    self.light_deferred.insert(sp);
                    self.deferred_rechecks.insert(sp);
                }
            }
        }
        self.deferred_rechecks.extend(affected.iter().copied());
        self.flush_settled_deferred_if_needed(target);

        // 7. Kick generated/overlaid water that now has somewhere to flow.
        self.queue_loaded_section_water_updates(&ingested);
        if !self.missing_columns_settled && self.extra_load_targets.is_empty() {
            self.request_missing_columns(target);
        }
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

    fn queue_deferred_rechecks_around(&mut self, pos: SectionPos) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    self.deferred_rechecks.insert(SectionPos::new(
                        pos.cx + dx,
                        pos.cy + dy,
                        pos.cz + dz,
                    ));
                }
            }
        }
    }

    fn queue_deferred_rechecks_around_column(&mut self, pos: ChunkPos) {
        for cz in pos.cz - 1..=pos.cz + 1 {
            for cx in pos.cx - 1..=pos.cx + 1 {
                for cy in Self::column_section_range() {
                    self.deferred_rechecks.insert(SectionPos::new(cx, cy, cz));
                }
            }
        }
    }

    /// Flush deferred sections whose generation neighbourhood has settled:
    /// request the single light bake (skipped when the section landed with
    /// clean persisted light) and queue the single first mesh. Sections whose
    /// saved overlay is still buffered stay parked so the bake reads the saved
    /// blocks, not the generated base it is about to replace.
    fn flush_settled_deferred_if_needed(&mut self, target: LoadTarget) {
        let check: Vec<SectionPos> = if self.deferred_recheck_needed {
            self.deferred_recheck_needed = false;
            self.deferred_rechecks.clear();
            self.light_deferred.iter().copied().collect()
        } else {
            std::mem::take(&mut self.deferred_rechecks)
                .into_iter()
                .filter(|sp| self.light_deferred.contains(sp))
                .collect()
        };
        if check.is_empty() {
            return;
        }
        self.flush_settled_deferred_positions(target, check);
    }

    #[cfg(test)]
    fn flush_settled_deferred(&mut self, target: LoadTarget) {
        let check = self.light_deferred.iter().copied().collect();
        self.flush_settled_deferred_positions(target, check);
    }

    fn flush_settled_deferred_positions(&mut self, target: LoadTarget, check: Vec<SectionPos>) {
        let ready: Vec<SectionPos> = check
            .into_iter()
            .filter(|sp| {
                !self.pending_overlays.contains_key(sp)
                    && self.gen_neighborhood_settled(*sp, target)
            })
            .collect();
        for sp in ready {
            let Some(section) = self.sections.get(&sp) else {
                self.light_deferred.remove(&sp);
                continue;
            };
            let needs_bake = section.light_dirty && !section.all_opaque();
            // Keep an enclosed mixed section parked, rather than forgetting its
            // first bake. A target move rechecks the set; once a player is close
            // enough to already be inside, proximity defeats the sealed skip.
            if needs_bake && self.section_sealed_by_loaded_neighbors(sp) {
                continue;
            }
            self.light_deferred.remove(&sp);
            // Clean light (persisted, loaded from disk) stands as-is.
            // Fully-opaque sections skip baking on both sides of the mesh pump's
            // light gate (their faces cull against solid cells and never sample light).
            if needs_bake {
                let key = target.section_priority_key(sp);
                self.light_bakes
                    .request(key, sp, &self.sections, &self.columns);
            }
            self.queue_dirty_mesh(sp);
        }
    }

    /// Whether the mob census is complete inside the spawn-relevant square around one
    /// player. Natural spawning only needs the population near the player it is trying
    /// to spawn around; unrelated far-edge streaming must not stop it.
    ///
    /// `radius` is Chebyshev chunk distance. The check intersects that square with the
    /// player's streamable disc so render distances smaller than the census radius can
    /// still become ready. Every column in that intersection must have landed, and no
    /// saved section record in the square may still be awaiting or buffering its mobs.
    pub fn mob_census_loaded_around(&self, center: ChunkPos, radius: i32) -> bool {
        let radius = radius.max(0);
        let stream_radius = self.render_dist.max(0);
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz > stream_radius * stream_radius {
                    continue;
                }
                let pos = ChunkPos::new(center.cx + dx, center.cz + dz);
                if !self.columns.contains_key(&pos) {
                    return false;
                }
            }
        }
        let in_neighborhood = |sp: &SectionPos| {
            let pos = sp.chunk_pos();
            let dx = pos.cx - center.cx;
            let dz = pos.cz - center.cz;
            dx.abs() <= radius
                && dz.abs() <= radius
                && dx * dx + dz * dz <= stream_radius * stream_radius
        };
        !self.awaited_overlays.iter().any(in_neighborhood)
            && !self.pending_overlays.keys().any(in_neighborhood)
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
    /// re-queues the contact, so no flow is permanently lost to gating. Four scans
    /// per ingested section, each cheap in the bulk cases:
    /// - water with air or FLOWING cells inside the section: the full interior scan
    ///   (shores, waterfalls, reloaded mid-flow water). A non-source cell always
    ///   re-arms — a flow check also re-levels and DRIES, and pending checks died
    ///   with the unload, so an enclosed mid-drain sheet would otherwise freeze at
    ///   flowing levels forever; a settled cell recomputes to itself and writes
    ///   nothing, so the kick stays cheap. Sources re-arm only next to loaded air
    ///   (spread is all they do);
    /// - all-source water without air (ocean interior, water over a sealed floor):
    ///   only the five outflow boundary planes, and only against a loaded neighbour
    ///   that holds air — calm open ocean skips every plane by summary;
    /// - any air: the five inflow boundary planes against loaded water-holding
    ///   neighbours, queueing the NEIGHBOUR's water cell — the cross-seam case
    ///   neither section's own water scan can see (its water, this section's air);
    /// - non-source water within `SIM_READ_REACH` of this section in every loaded
    ///   neighbour: while this section was absent, the guard DROPPED fired checks
    ///   whose read box touched it — checks living up to `SIM_READ_REACH` cells
    ///   inside sections that never unloaded, which no ingested-section scan sees.
    ///   All-source neighbours (calm ocean) skip by the metadata summary.
    pub(super) fn queue_loaded_section_water_updates(&mut self, ingested: &[SectionPos]) {
        const REACH: usize = super::sim_guard::SIM_READ_REACH as usize;
        let air = Block::Air.id();
        let water = Block::Water.id();
        let ingested_set: FxHashSet<SectionPos> = ingested.iter().copied().collect();
        let mut updates: Vec<IVec3> = Vec::new();
        for sp in ingested {
            let Some(section) = self.sections.get(sp) else {
                continue;
            };
            let (ox, oy, oz) = sp.origin_world();
            let has_water = section.has_water();
            let has_air = section.has_air();
            let has_flowing = section
                .water_slice()
                .is_some_and(|metas| metas.iter().any(|&m| m != 0));

            if has_water && (has_air || has_flowing) {
                let blocks = section.blocks_slice();
                let metas = section.water_slice();
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            let idx = section_idx(lx, ly, lz);
                            if blocks[idx] != water {
                                continue;
                            }
                            let wx = ox + lx as i32;
                            let wy = oy + ly as i32;
                            let wz = oz + lz as i32;
                            if metas.is_some_and(|m| m[idx] != 0) {
                                updates.push(IVec3::new(wx, wy, wz));
                                continue;
                            }
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

            // Guard-drop re-arm (doc above): non-source water in the loaded,
            // non-ingested 26-neighbourhood within read reach of this section.
            let local_range = |d: i32| match d {
                -1 => SECTION_SIZE - REACH..SECTION_SIZE,
                1 => 0..REACH,
                _ => 0..SECTION_SIZE,
            };
            for ndy in -1..=1i32 {
                for ndz in -1..=1i32 {
                    for ndx in -1..=1i32 {
                        if ndx == 0 && ndy == 0 && ndz == 0 {
                            continue;
                        }
                        let npos = SectionPos::new(sp.cx + ndx, sp.cy + ndy, sp.cz + ndz);
                        if ingested_set.contains(&npos) {
                            continue; // its own interior scan covers it fully
                        }
                        let Some(ns) = self.sections.get(&npos) else {
                            continue;
                        };
                        let Some(metas) = ns.water_slice() else {
                            continue; // all sources: nothing mid-flow to re-arm
                        };
                        let nblocks = ns.blocks_slice();
                        let (nox, noy, noz) = npos.origin_world();
                        for ly in local_range(ndy) {
                            for lz in local_range(ndz) {
                                for lx in local_range(ndx) {
                                    let idx = section_idx(lx, ly, lz);
                                    if metas[idx] != 0 && nblocks[idx] == water {
                                        updates.push(IVec3::new(
                                            nox + lx as i32,
                                            noy + ly as i32,
                                            noz + lz as i32,
                                        ));
                                    }
                                }
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
        saved.set_block(0, 0, 0, crate::block::Block::Chest);
        saved.insert_container(
            0,
            0,
            0,
            crate::container::Container::with_len(crate::world::chest::CHEST_SLOTS),
        );
        saved.insert_entity_facing(0, 0, 0, crate::facing::Facing::default());
        world
            .pending_overlays
            .insert(sp, (saved, Vec::new(), Vec::new()));
        world.apply_pending_overlays();

        let mut out = Vec::new();
        world.collect_chests(&mut out);
        assert_eq!(out.len(), 1, "the overlaid chest must be collected");
    }

    /// The spawn census waits only for the nearby streamable neighborhood: a saved
    /// mob record there must block caps, while unrelated far streaming must not.
    #[test]
    fn mob_census_waits_for_nearby_columns_and_overlays_only() {
        let mut world = World::new(0, 2);
        let center = ChunkPos::new(0, 0);
        let census_radius = 9;

        for dz in -2..=2 {
            for dx in -2..=2 {
                if dx * dx + dz * dz <= 4 {
                    world.insert_empty_column_for_test(ChunkPos::new(dx, dz));
                }
            }
        }
        assert!(
            world.mob_census_loaded_around(center, census_radius),
            "every streamable nearby column is loaded"
        );

        let near = SectionPos::new(1, 4, 0);
        world.awaited_overlays.insert(near);
        assert!(!world.mob_census_loaded_around(center, census_radius));
        world.awaited_overlays.clear();

        let far = SectionPos::new(20, 4, 0);
        world.awaited_overlays.insert(far);
        assert!(
            world.mob_census_loaded_around(center, census_radius),
            "far streaming does not block the local census"
        );

        world.remove_column(ChunkPos::new(0, 1));
        assert!(
            !world.mob_census_loaded_around(center, census_radius),
            "a missing nearby column closes the gate"
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

    /// Two distant anchors: `update_load_multi` must request BOTH
    /// neighbourhoods (nothing outside the union) and keep loaded content near
    /// each anchor while evicting what no anchor wants.
    #[test]
    fn multi_anchor_requests_and_keeps_both_neighbourhoods() {
        let mut world = World::new(0, 4);
        let a = LoadAnchor {
            cx: 0,
            cy: 4,
            cz: 0,
            radius: 64,
        };
        let b = LoadAnchor {
            cx: 40,
            cy: 4,
            cz: 0,
            radius: 64,
        };
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.insert_empty_column_for_test(ChunkPos::new(40, 0));
        world.insert_empty_column_for_test(ChunkPos::new(20, 0)); // far from both

        world.update_load_multi(&[a, b]);

        let near = |p: &ChunkPos, cx: i32| (p.cx - cx).abs() <= 4 && p.cz.abs() <= 4;
        assert!(
            world.pending.keys().any(|p| near(p, 0)),
            "anchor A's columns are requested"
        );
        assert!(
            world.pending.keys().any(|p| near(p, 40)),
            "anchor B's columns are requested"
        );
        assert!(
            world.pending.keys().all(|p| near(p, 0) || near(p, 40)),
            "nothing outside the anchors' union is requested"
        );

        assert!(world.chunk_loaded(0, 0), "anchor A's column is kept");
        assert!(world.chunk_loaded(40, 0), "anchor B's column is kept");
        assert!(
            !world.chunk_loaded(20, 0),
            "a column no anchor keeps is evicted"
        );
    }

    /// The settled short-circuit skips the per-pump missing-column rescan but
    /// must never hide a column that became missing WITHOUT an anchor change
    /// (eviction, failed gen job) — a stale flag here means terrain that never
    /// loads again while the player stands still.
    #[test]
    fn settled_missing_scan_resumes_after_eviction() {
        let mut world = World::new(0, 4);
        // Repeated same-target updates submit the whole wanted disc (64 per
        // call) and then settle.
        for _ in 0..100 {
            world.update_load(0, 4, 0);
            if world.missing_columns_settled {
                break;
            }
        }
        assert!(
            world.missing_columns_settled,
            "a fully requested disc settles the scan"
        );
        let victim = ChunkPos::new(0, 0);
        assert!(
            world.pending.contains_key(&victim) || world.column_gen.contains_key(&victim),
            "the player's own column is requested or loaded"
        );

        // A column dropped without any anchor change must be re-found by the
        // next same-target scan.
        world.remove_column(victim);
        assert!(
            !world.missing_columns_settled,
            "eviction un-settles the scan"
        );
        world.update_load(0, 4, 0);
        assert!(
            world.pending.contains_key(&victim),
            "the evicted column is re-requested by the next scan"
        );
    }

    /// One anchor through `update_load_multi` IS `update_load`: same
    /// target, same requested set, no multi-anchor residue.
    #[test]
    fn single_anchor_multi_load_matches_update_load() {
        let mut single = World::new(0x51EED, 3);
        let mut multi = World::new(0x51EED, 3);
        single.update_load(2, 5, -1);
        multi.update_load_multi(&[LoadAnchor {
            cx: 2,
            cy: 5,
            cz: -1,
            radius: 64,
        }]);

        assert_eq!(single.last_load_target, multi.last_load_target);
        assert!(multi.extra_load_targets.is_empty());
        let sorted = |w: &World| {
            let mut p: Vec<ChunkPos> = w.pending.keys().copied().collect();
            p.sort_by_key(|c| (c.cx, c.cz));
            p
        };
        assert_eq!(
            sorted(&single),
            sorted(&multi),
            "one anchor must request exactly the update_load set"
        );
    }

    #[test]
    fn streaming_wants_a_full_horizontal_disc() {
        let target = LoadTarget::new(0, 5, 0, 16);

        assert!(
            World::column_wanted(target, ChunkPos::new(10, 0)),
            "positive X is wanted"
        );
        assert!(
            World::column_wanted(target, ChunkPos::new(-10, 0)),
            "equal-distance negative X is wanted"
        );
        assert!(
            World::column_wanted(target, ChunkPos::new(0, 16)),
            "the circular boundary is included"
        );
        assert!(
            !World::column_wanted(target, ChunkPos::new(12, 12)),
            "the square corner outside the disc is excluded"
        );
        assert!(
            !World::column_kept(target, ChunkPos::new(-20, 0)),
            "columns beyond circular unload hysteresis are evicted"
        );
    }

    #[test]
    fn streaming_priority_is_distance_only() {
        let target = LoadTarget::new(0, 5, 0, 16);

        assert!(
            target.column_priority_key(ChunkPos::new(0, 2))
                < target.column_priority_key(ChunkPos::new(16, 0)),
            "near terrain must beat the far edge"
        );
        assert_eq!(
            target.column_priority_key(ChunkPos::new(6, 0)),
            target.column_priority_key(ChunkPos::new(-6, 0)),
            "opposite directions at the same distance have equal priority"
        );
        assert_eq!(
            target.column_priority_key(ChunkPos::new(6, 0)),
            target.column_priority_key(ChunkPos::new(0, 6)),
            "axes at the same distance have equal priority"
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
    fn sealed_first_light_waits_for_player_proximity_then_bakes() {
        let mut world = World::new(0, 0);
        let center = SectionPos::new(0, 0, 0);
        let mut cavity = Section::new(0, 0, 0);
        cavity.blocks_slice_mut().fill(Block::Stone.id());
        cavity.recompute_opaque_count();
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
            let pos = SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz);
            let mut section = Section::new(pos.cx, pos.cy, pos.cz);
            section.blocks_slice_mut().fill(Block::Stone.id());
            section.recompute_opaque_count();
            world.insert_section_for_test(pos, section);
        }
        let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
        world.column_gen.insert(
            center.chunk_pos(),
            Arc::new(generator.generate_column_gen(center.cx, center.cz)),
        );

        let far = LoadTarget::new(8, 0, 0, 0);
        world.last_load_target = Some(far);
        world.light_deferred.insert(center);
        world.flush_settled_deferred(far);
        assert!(
            world.light_deferred.contains(&center),
            "an unreachable sealed cavity can leave its first light deferred"
        );
        assert!(!world.light_bakes.has_pending());

        let near = LoadTarget::new(0, 0, 0, 0);
        world.last_load_target = Some(near);
        world.flush_settled_deferred(near);
        assert!(!world.light_deferred.contains(&center));
        assert!(
            world.light_bakes.has_pending(),
            "approaching the cavity must wake its first light bake"
        );
    }

    #[test]
    fn stale_pending_columns_are_pruned_to_current_disc() {
        let mut world = World::new(0, 16);
        let outside = ChunkPos::new(17, 0);
        let inside = ChunkPos::new(-10, 0);
        world.pending.insert(outside, None);
        world.pending.insert(inside, None);

        let target = LoadTarget::new(0, 5, 0, 16);
        world.prune_stale_column_requests(target);

        assert!(
            !world.pending.contains_key(&outside),
            "queued work outside the disc should be dropped"
        );
        assert!(
            world.pending.contains_key(&inside),
            "queued work inside the disc stays queued"
        );
    }

    #[test]
    fn horizontal_move_requests_sections_for_newly_wanted_loaded_columns() {
        use std::sync::Arc;

        let mut world = World::new(0x51EED, 8);
        let old = LoadTarget::new(0, 5, 0, 8);
        let newly_wanted = ChunkPos::new(9, 0);
        assert!(
            !World::column_wanted(old, newly_wanted),
            "test setup: column starts outside the old disc"
        );

        let generator = crate::worldgen::driver::ChunkGenerator::new(world.seed);
        let col = Arc::new(generator.generate_column_gen(newly_wanted.cx, newly_wanted.cz));
        world.column_gen.insert(newly_wanted, col);
        world.last_load_target = Some(old);

        world.update_load(1, 5, 0);

        assert!(
            world
                .pending_sections
                .iter()
                .any(|sp| sp.chunk_pos() == newly_wanted),
            "a generated column that enters the disc must request its sections"
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

        let dir = std::env::temp_dir().join(format!("petramond-cubic-e2e-{}", std::process::id()));
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
            save.request_load(sp, false);
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

    /// "Optimize explored terrain" end to end: a first visit persists every
    /// explored section AND the column-gen cache on flush; a reload of the same
    /// area installs everything from disk — every stream event is `Loaded`,
    /// none `Generated` — with content identical to the first visit.
    #[cfg(feature = "worldgen-tests")]
    #[test]
    fn optimize_explored_terrain_reloads_from_disk_without_generating() {
        use std::time::Duration;

        let dir =
            std::env::temp_dir().join(format!("petramond-optimize-terrain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let stream_settled = |world: &mut World| {
            world.update_load(0, 8, 0);
            let mut settled = 0;
            let mut last = 0usize;
            for _ in 0..5000 {
                world.poll();
                let now = world.loaded_section_count();
                if now == last && now > 0 {
                    settled += 1;
                    if settled >= 100 {
                        break;
                    }
                } else {
                    settled = 0;
                    last = now;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        };

        // Every section's light settled: baked-and-clean, or fully opaque
        // (never bakes). The first-persist gate waits for exactly this.
        let light_settled = |world: &mut World| {
            for _ in 0..5000 {
                world.poll();
                world.pump_light_bakes();
                let done = world
                    .sections
                    .values()
                    .all(|s| !s.light_dirty || s.all_opaque());
                if done {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            false
        };

        // First visit: generate, then flush (autosave path) — the flag persists
        // every explored section and the column-gen cache.
        let opened = crate::save::open_at(dir.clone()).expect("open save");
        let mut world = World::new(0x51EED, 2);
        world.attach_save(opened.save);
        world.set_optimize_explored_terrain(true);
        stream_settled(&mut world);
        assert!(light_settled(&mut world), "first-visit light bakes settle");
        let first_sections: Vec<SectionPos> = world.sections.keys().copied().collect();
        assert!(!first_sections.is_empty());
        let first_blocks: std::collections::HashMap<SectionPos, Vec<u8>> = first_sections
            .iter()
            .map(|sp| (*sp, world.sections[sp].blocks_slice().to_vec()))
            .collect();
        world.flush_modified_chunks();
        {
            let save = world.save().expect("save attached");
            for sp in &first_sections {
                assert!(
                    save.manifest_contains(*sp),
                    "explored section {sp:?} must persist with the flag on"
                );
            }
            assert!(
                save.colgen_manifest_contains(ChunkPos::new(0, 0)),
                "explored columns must enter the column-gen cache"
            );
        }
        drop(world); // joins the save thread: everything is on disk.

        // Reload: same area must come back entirely from disk.
        let opened = crate::save::open_at(dir.clone()).expect("reopen save");
        let mut world = World::new(0x51EED, 2);
        world.attach_save(opened.save);
        world.set_optimize_explored_terrain(true);
        world.set_stream_event_capture(true);
        stream_settled(&mut world);

        let events = world.take_stream_events();
        let generated = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Generated(_)))
            .count();
        let loaded = events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Loaded(_)))
            .count();
        assert_eq!(
            generated, 0,
            "explored terrain must not regenerate on reload ({loaded} loaded)"
        );
        assert!(loaded > 0, "sections came back from disk");
        for (sp, blocks) in &first_blocks {
            let section = world
                .sections
                .get(sp)
                .unwrap_or_else(|| panic!("section {sp:?} reloaded"));
            assert_eq!(
                section.blocks_slice(),
                &blocks[..],
                "reloaded content diverged at {sp:?}"
            );
        }

        // Light persistence: every reloaded section came back with its saved
        // cubes ALREADY CLEAN — nothing above ever drained a bake for the
        // reloaded world, so a single dirty section here would mean the load
        // path re-queued a bake (the exact work persistence exists to skip).
        let relit = world
            .sections
            .values()
            .filter(|s| s.light_dirty && !s.all_opaque())
            .count();
        assert_eq!(
            relit, 0,
            "reloaded sections must keep their persisted light without re-baking"
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
