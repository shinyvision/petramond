use crate::world::ServerWorld;
use rustc_hash::FxHashSet;
use std::sync::Arc;

use crate::worker::GenJob;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_worldgen::ColumnGen;

use crate::world::store::{LoadAnchor, LoadTarget};

/// Bound column work while allowing nearer requests to replace waiting jobs
/// when the anchors move.
pub(super) const MAX_PENDING_COLUMN_GEN_JOBS: usize = 192;
const MAX_COLUMN_GEN_SUBMITS_PER_TARGET: usize = 64;
/// Includes worker generation and disk-primary loads. Keeping the whole stage
/// below this ceiling also bounds the worldgen closures retained by JobPool.
pub(super) const MAX_PENDING_SECTION_JOBS: usize = 512;

impl ServerWorld {
    /// Update the streamed region around the player's SECTION `(cam_chunk_x, cam_chunk_y,
    /// cam_chunk_z)`. The world streams a flattened cylinder: a Euclidean horizontal disc
    /// of columns, each loaded only across a vertical window of sections around the player
    /// (see `VERTICAL_LOAD_RADIUS`). Generation is per 16³ section, prioritised by 3D
    /// distance — "worldgen closest to the player" — so the deep underground / high sky a
    /// far column doesn't need is never generated until the player approaches it (room for
    /// caves below y=0). Scans are gated to player-section / render-distance changes; call
    /// `poll` every frame to keep ingesting worker results.
    pub fn update_load(&mut self, cam_chunk_x: i32, cam_chunk_y: i32, cam_chunk_z: i32) {
        self.side.gen.section_submit_budget = super::MAX_SECTION_GEN_SUBMITS_PER_PHASE;
        let target = LoadTarget::new(cam_chunk_x, cam_chunk_y, cam_chunk_z, self.data.render_dist);
        self.update_load_target(target);
    }

    fn update_load_target(&mut self, target: LoadTarget) {
        // The single-anchor path: any multi-anchor residue is gone.
        let anchors_changed = !self.data.extra_load_targets.is_empty();
        self.data.extra_load_targets.clear();
        if !anchors_changed && self.data.last_load_target == Some(target) {
            if !self.data.missing_columns_settled {
                self.request_missing_columns(target);
            }
            return;
        }
        let prev = self.data.last_load_target.filter(|_| !anchors_changed);
        self.data.last_load_target = Some(target);
        self.data.missing_columns_settled = false;
        self.data.deferred_recheck_needed = true;
        let vertical_moved = prev.is_none_or(|p| p.center_cy != target.center_cy);
        let horizontal_keep_changed =
            prev.is_none_or(|p| p.center != target.center || p.render_dist != target.render_dist);

        self.prune_stale_column_requests(target);
        self.prune_stale_section_requests();
        self.reclaim_far_column_requests();
        self.refresh_generation_priorities();
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
    /// `update_load` including its incremental rescan optimizations; the N ≥ 2 path
    /// trades those delta scans for a plain full scan on anchor-set change (bounded by
    /// the anchors' discs, and it runs only on change).
    pub fn update_load_multi(&mut self, anchors: &[LoadAnchor]) {
        self.side.gen.section_submit_budget = super::MAX_SECTION_GEN_SUBMITS_PER_PHASE;
        // Each anchor streams at ITS connection's radius (view distance),
        // never wider than this world's own `render_dist` budget.
        let radius = |a: &LoadAnchor| a.radius.clamp(1, self.data.render_dist);
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
            self.data.last_load_target == Some(targets[0]) && self.data.extra_load_targets == targets[1..];
        if unchanged {
            if !self.data.missing_columns_settled {
                self.request_missing_columns_multi(&targets);
            }
            return;
        }
        self.data.last_load_target = Some(targets[0]);
        self.data.extra_load_targets = targets[1..].to_vec();
        self.data.missing_columns_settled = false;
        self.data.deferred_recheck_needed = true;
        self.side.gen.pending.retain(|pos, job| {
            let keep = targets.iter().any(|t| Self::column_wanted(*t, *pos));
            if !keep {
                if let Some(job) = job {
                    job.cancel();
                }
            }
            keep
        });
        self.reclaim_far_column_requests();
        self.prune_stale_section_requests();
        self.refresh_generation_priorities();
        self.request_missing_columns_multi(&targets);
        self.request_wanted_sections_multi(&targets);
        self.unload_far_multi(&targets);
    }

    fn multi_column_key(targets: &[LoadTarget], pos: ChunkPos) -> i64 {
        targets
            .iter()
            .map(|t| t.column_priority_key(pos))
            .min()
            .expect("at least one target")
    }

    fn multi_biased_section_key(
        targets: &[LoadTarget],
        underground: &[bool],
        sp: SectionPos,
        band_lo: i32,
    ) -> i64 {
        targets
            .iter()
            .zip(underground)
            .map(|(t, &u)| t.surface_biased_section_key(sp, band_lo, u))
            .min()
            .expect("at least one target")
    }

    /// [`request_missing_columns`] over the union of the anchors' discs.
    fn request_missing_columns_multi(&mut self, targets: &[LoadTarget]) {
        let submit_limit = MAX_COLUMN_GEN_SUBMITS_PER_TARGET
            .min(MAX_PENDING_COLUMN_GEN_JOBS.saturating_sub(self.side.gen.pending.len()));
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
                    if self.side.gen.column_gen.contains_key(&pos) || self.side.gen.pending.contains_key(&pos)
                    {
                        continue;
                    }
                    missing.push((Self::multi_column_key(targets, pos), pos));
                }
            }
        }
        // Same settled short-circuit as the single-anchor scan.
        self.data.missing_columns_settled = missing.len() <= submit_limit;
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.submit_column_job(priority, pos);
        }
    }

    /// [`request_wanted_sections`] with each column's wanted window = the
    /// union over the anchors that want it.
    fn request_wanted_sections_multi(&mut self, targets: &[LoadTarget]) {
        let underground: Vec<bool> = targets
            .iter()
            .map(|t| self.anchor_underground(*t))
            .collect();
        let mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)> = Vec::new();
        let mut cys: Vec<i32> = Vec::new();
        for (pos, col) in &self.side.gen.column_gen {
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
            let band_lo = *Self::surface_window_for_column(col, 0).start();
            for &cy in &cys {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if self.data.sections.contains_key(&sp) || self.side.gen.pending_sections.contains(&sp) {
                    continue;
                }
                if self.skip_empty_sky_section(sp, content_top) {
                    continue;
                }
                wanted.push((
                    Self::multi_biased_section_key(targets, &underground, sp, band_lo),
                    sp,
                    col.clone(),
                ));
            }
        }
        self.admit_section_candidates(wanted);
    }

    /// Submit the (heavy, once-per-column) `ColumnGen` job for every in-radius column we
    /// have neither loaded nor queued, NEAREST-FIRST so the player's surroundings resolve
    /// first. Each landed column then drives its own per-section jobs (`poll`).
    pub(super) fn request_missing_columns(&mut self, target: LoadTarget) {
        let submit_limit = MAX_COLUMN_GEN_SUBMITS_PER_TARGET
            .min(MAX_PENDING_COLUMN_GEN_JOBS.saturating_sub(self.side.gen.pending.len()));
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
                if self.side.gen.column_gen.contains_key(&pos) || self.side.gen.pending.contains_key(&pos) {
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
        if self.data.extra_load_targets.is_empty() {
            self.data.missing_columns_settled = missing.len() <= submit_limit;
        }
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.submit_column_job(priority, pos);
        }
    }

    /// Queue one column's gen job (or its column-gen cache read) and mark it
    /// pending. Shared by the single- and multi-anchor request scans.
    fn submit_column_job(&mut self, priority: i64, pos: ChunkPos) {
        // An explored column's 2D gen data loads from the column-gen cache
        // instead of running the heavy noise job; a decode miss falls back
        // to the worker (poll's cache drain resubmits). Both answers resolve
        // the same `pending` entry.
        let cached = self
            .side.save
            .as_ref()
            .is_some_and(|s| s.colgen_manifest_contains(pos));
        let job = if cached {
            if let Some(save) = self.side.save.as_ref() {
                save.request_column_gen(pos, self.data.seed);
            }
            None
        } else {
            Some(self.side.worker.submit(
                priority,
                GenJob::Column {
                    pos,
                    seed: self.data.seed,
                },
            ))
        };
        self.side.gen.pending.insert(pos, job);
    }

    pub(super) fn prune_stale_column_requests(&mut self, target: LoadTarget) {
        self.side.gen.pending.retain(|pos, job| {
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
        let underground = self.anchor_underground(target);
        let prev_window = Self::vertical_window(prev.center_cy, 0);
        let mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)> = Vec::new();
        let mut cys: Vec<i32> = Vec::new();
        for (pos, col) in &self.side.gen.column_gen {
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
            for &cy in self.data.saved.sections_in_column(*pos) {
                if !cys.contains(&cy) {
                    cys.push(cy);
                }
            }
            let content_top = col.content_top();
            let band_lo = *Self::surface_window_for_column(col, 0).start();
            for &cy in &cys {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if self.data.sections.contains_key(&sp) || self.side.gen.pending_sections.contains(&sp) {
                    continue;
                }
                if self.skip_empty_sky_section(sp, content_top) {
                    continue;
                }
                wanted.push((
                    target.surface_biased_section_key(sp, band_lo, underground),
                    sp,
                    col.clone(),
                ));
            }
        }
        self.admit_section_candidates(wanted);
    }

    fn request_newly_wanted_sections(&mut self, prev: LoadTarget, target: LoadTarget) {
        self.request_wanted_sections_matching(target, |pos| !Self::column_wanted(prev, pos));
    }

    fn request_wanted_sections_matching(
        &mut self,
        target: LoadTarget,
        mut include_column: impl FnMut(ChunkPos) -> bool,
    ) {
        let underground = self.anchor_underground(target);
        let center_cy = target.center_cy;
        let mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)> = Vec::new();
        for (pos, col) in &self.side.gen.column_gen {
            if !Self::column_wanted(target, *pos) || !include_column(*pos) {
                continue;
            }
            let band_lo = *Self::surface_window_for_column(col, 0).start();
            for cy in self.wanted_section_cys_for_column(*pos, col, center_cy, 0) {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if self.data.sections.contains_key(&sp) || self.side.gen.pending_sections.contains(&sp) {
                    continue;
                }
                if self.skip_empty_sky_section(sp, col.content_top()) {
                    continue;
                }
                wanted.push((
                    target.surface_biased_section_key(sp, band_lo, underground),
                    sp,
                    col.clone(),
                ));
            }
        }
        self.admit_section_candidates(wanted);
    }

    /// Submit per-section gen jobs for one freshly-loaded column's vertical window
    /// (nearest the player's `cy` first), so a column starts filling the moment its
    /// shared data lands without waiting for the next `update_load`.
    pub(super) fn request_sections_for_column(&mut self, pos: ChunkPos, target: LoadTarget) {
        let Some(col) = self.side.gen.column_gen.get(&pos).cloned() else {
            return;
        };
        let underground = self.anchor_underground(target);
        let mut wanted: Vec<(i64, SectionPos)> = Vec::new();
        let content_top = col.content_top();
        let band_lo = *Self::surface_window_for_column(&col, 0).start();
        for cy in self.wanted_section_cys_for_column(pos, &col, target.center_cy, 0) {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if self.data.sections.contains_key(&sp) || self.side.gen.pending_sections.contains(&sp) {
                continue;
            }
            if self.skip_empty_sky_section(sp, content_top) {
                continue;
            }
            // The full 3D key (not just dcy²): these compete in the shared pool
            // against other columns' sections, so the key must be globally comparable.
            wanted.push((
                target.surface_biased_section_key(sp, band_lo, underground),
                sp,
            ));
        }
        self.admit_section_candidates(
            wanted.into_iter().map(|(key, sp)| (key, sp, col.clone())).collect(),
        );
    }

    /// Admit the nearest missing section requests under both the in-flight cap
    /// and this pump's section quota. Remaining positions are not marked
    /// pending: a later poll re-derives them from the loaded column data after
    /// existing jobs drain, so no unbounded deferred queue is retained.
    fn admit_section_candidates(&mut self, mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)>) {
        if wanted.is_empty() {
            return;
        }
        wanted.sort_unstable_by_key(|(key, sp, _)| (*key, sp.cx, sp.cz, sp.cy));
        let room = MAX_PENDING_SECTION_JOBS
            .saturating_sub(self.side.gen.pending_sections.len())
            .min(self.side.gen.section_submit_budget);
        if wanted.len() > room {
            self.side.gen.section_requests_unsettled = true;
        }
        for (key, sp, col) in wanted.into_iter().take(room) {
            self.submit_section_job(key, sp, col);
            self.side.gen.section_submit_budget -= 1;
        }
    }

    /// Refill after a bounded pass omitted sections or a section job failed.
    /// This full scan runs only while the stage is unsettled and has room; it
    /// restores nearest-first ordering across every already landed column.
    pub(super) fn refill_section_requests(&mut self) {
        if !self.side.gen.section_requests_unsettled
            || self.side.gen.pending_sections.len() >= MAX_PENDING_SECTION_JOBS
            || self.side.gen.section_submit_budget == 0
        {
            return;
        }
        self.side.gen.section_requests_unsettled = false;
        let Some(primary) = self.data.last_load_target else {
            return;
        };
        if self.data.extra_load_targets.is_empty() {
            self.request_wanted_sections(primary);
        } else {
            let targets: Vec<_> = std::iter::once(primary)
                .chain(self.data.extra_load_targets.iter().copied())
                .collect();
            self.request_wanted_sections_multi(&targets);
        }
    }

    /// Queue one section's gen job and, paired with it, ask the save thread for that
    /// section's saved (player-modified) record if one exists — so the disk overlay
    /// lands after the generated base and wins (`apply_pending_overlays`).
    ///
    /// A section that exists on disk skips the gen job entirely: its record is a
    /// full section that would have replaced the generated base anyway, so it
    /// installs as the PRIMARY content when the save thread answers (`poll`), and
    /// generation runs only as the corrupt-record fallback.
    fn submit_section_job(&mut self, key: i64, sp: SectionPos, col: Arc<ColumnGen>) {
        let disk_primary = self.data.saved_section_contains(sp);
        if disk_primary {
            self.insert_pending_section(sp);
            self.side.gen.disk_primary_sections.insert(sp);
            // The section's true content is in flight until the save thread
            // answers: the sim guard blocks mutation and the harvest skips
            // persisting it meanwhile (same contract as the overlay path).
            self.side.gen.awaited_overlays.insert(sp);
            self.note_stream_nonfinal(sp);
            if let Some(save) = self.side.save.as_ref() {
                save.request_load(&self.data.saved, sp, true);
            }
            return;
        }
        let job = self.side.worker.submit(
            key,
            GenJob::Section {
                sp,
                col,
                seed: self.data.seed,
            },
        );
        self.insert_pending_section(sp);
        self.side.gen.pending_section_jobs.insert(sp, job);
        if let Some(save) = self.side.save.as_ref() {
            if self.data.saved.authoritative_contains(sp) {
                save.request_load(&self.data.saved, sp, false);
                // The section's true content is now in flight until the save thread
                // answers (and the overlay applies): the sim guard blocks mutation
                // and the harvest skips persisting it meanwhile.
                self.side.gen.awaited_overlays.insert(sp);
                self.note_stream_nonfinal(sp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::JobPool;
    use petramond_worldgen::driver::ChunkGenerator;

    #[test]
    fn section_admission_stops_at_the_cap_and_retains_refill_intent() {
        let pool = Arc::new(JobPool::new(1));
        let (release, held) = std::sync::mpsc::channel();
        pool.submit(i64::MIN, move || {
            let _ = held.recv();
        });
        let mut world = ServerWorld::with_pool(7, 4, pool);
        let col = Arc::new(ChunkGenerator::new(7).generate_column_gen(0, 0));
        let wanted: Vec<_> = (0..=MAX_PENDING_SECTION_JOBS)
            .map(|i| {
                let sp = SectionPos::new((i / 16) as i32, (i % 16) as i32, 0);
                (i as i64, sp, col.clone())
            })
            .collect();

        world.side.gen.section_submit_budget = MAX_PENDING_SECTION_JOBS;
        world.admit_section_candidates(wanted.clone());
        assert_eq!(world.side.gen.pending_sections.len(), MAX_PENDING_SECTION_JOBS);
        assert!(world.side.gen.section_requests_unsettled);
        let last = wanted.last().unwrap().1;
        assert!(!world.side.gen.pending_sections.contains(&last));

        for job in world.side.gen.pending_section_jobs.values() {
            job.cancel();
        }
        release.send(()).unwrap();
    }

    #[test]
    fn omitted_sections_refill_without_an_anchor_move() {
        let pool = Arc::new(JobPool::new(1));
        let (release, held) = std::sync::mpsc::channel();
        pool.submit(i64::MIN, move || {
            let _ = held.recv();
        });
        let mut world = ServerWorld::with_pool(7, 4, pool);
        let target = LoadTarget::new(0, 4, 0, 4);
        world.data.last_load_target = Some(target);
        world.side.gen.column_gen.insert(
            target.center,
            Arc::new(ChunkGenerator::new(7).generate_column_gen(0, 0)),
        );

        world.side.gen.section_submit_budget = 1;
        world.request_wanted_sections(target);
        assert_eq!(world.side.gen.pending_sections.len(), 1);
        assert!(world.side.gen.section_requests_unsettled);

        world.side.gen.section_submit_budget = super::super::MAX_SECTION_GEN_SUBMITS_PER_PHASE;
        world.refill_section_requests();
        assert!(world.side.gen.pending_sections.len() > 1);
        assert!(!world.side.gen.section_requests_unsettled);

        for job in world.side.gen.pending_section_jobs.values() {
            job.cancel();
        }
        release.send(()).unwrap();
    }
}
