use crate::world::ServerWorld;
use rustc_hash::FxHashSet;
use std::sync::Arc;

use crate::worker::GenJob;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_worldgen::ColumnGen;

use crate::world::store::{LoadAnchor, LoadTarget};

pub(super) const MAX_PENDING_COLUMN_GEN_JOBS: usize = 192;
const MAX_COLUMN_GEN_SUBMITS_PER_TARGET: usize = 64;

impl ServerWorld {
    pub fn update_load(&mut self, cam_chunk_x: i32, cam_chunk_y: i32, cam_chunk_z: i32) {
        let target = LoadTarget::new(cam_chunk_x, cam_chunk_y, cam_chunk_z, self.data.render_dist);
        self.update_load_target(target);
    }

    fn update_load_target(&mut self, target: LoadTarget) {
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

    pub fn update_load_multi(&mut self, anchors: &[LoadAnchor]) {
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
        let unchanged = self.data.last_load_target == Some(targets[0])
            && self.data.extra_load_targets == targets[1..];
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
                    if self.side.gen.column_gen.contains_key(&pos)
                        || self.side.gen.pending.contains_key(&pos)
                    {
                        continue;
                    }
                    missing.push((Self::multi_column_key(targets, pos), pos));
                }
            }
        }
        self.data.missing_columns_settled = missing.len() <= submit_limit;
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.submit_column_job(priority, pos);
        }
    }

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
                if self.data.sections.contains_key(&sp)
                    || self.side.gen.pending_sections.contains(&sp)
                {
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
                if self.side.gen.column_gen.contains_key(&pos)
                    || self.side.gen.pending.contains_key(&pos)
                {
                    continue;
                }
                missing.push((target.column_priority_key(pos), pos));
            }
        }
        if self.data.extra_load_targets.is_empty() {
            self.data.missing_columns_settled = missing.len() <= submit_limit;
        }
        missing.sort_by_key(|(priority, _)| *priority);
        for (priority, pos) in missing.into_iter().take(submit_limit) {
            self.submit_column_job(priority, pos);
        }
    }

    fn submit_column_job(&mut self, priority: i64, pos: ChunkPos) {
        let cached = self
            .side
            .save
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
                if self.data.sections.contains_key(&sp)
                    || self.side.gen.pending_sections.contains(&sp)
                {
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
                if self.data.sections.contains_key(&sp)
                    || self.side.gen.pending_sections.contains(&sp)
                {
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
            if self.data.sections.contains_key(&sp) || self.side.gen.pending_sections.contains(&sp)
            {
                continue;
            }
            if self.skip_empty_sky_section(sp, content_top) {
                continue;
            }
            wanted.push((
                target.surface_biased_section_key(sp, band_lo, underground),
                sp,
            ));
        }
        self.admit_section_candidates(
            wanted
                .into_iter()
                .map(|(key, sp)| (key, sp, col.clone()))
                .collect(),
        );
    }

    pub(super) fn admit_section_candidates(
        &mut self,
        mut wanted: Vec<(i64, SectionPos, Arc<ColumnGen>)>,
    ) {
        wanted.sort_unstable_by_key(|(key, sp, _)| (*key, sp.cx, sp.cz, sp.cy));
        for (key, sp, col) in wanted {
            self.submit_section_job(key, sp, col);
        }
    }

    pub(super) fn refill_section_requests(&mut self) {
        if !self.side.gen.section_requests_unsettled {
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

    fn submit_section_job(&mut self, key: i64, sp: SectionPos, col: Arc<ColumnGen>) {
        let disk_primary = self.data.saved_section_contains(sp);
        if disk_primary {
            self.insert_pending_section(sp);
            self.side.gen.disk_primary_sections.insert(sp);
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
    fn every_wanted_section_is_submitted_at_once() {
        let pool = Arc::new(JobPool::new(1));
        let (release, held) = std::sync::mpsc::channel();
        pool.submit(i64::MIN, move || {
            let _ = held.recv();
        });
        let mut world = ServerWorld::with_pool(7, 4, pool);
        let col = Arc::new(ChunkGenerator::new(7).generate_column_gen(0, 0));
        let wanted: Vec<_> = (0..2048)
            .map(|i| {
                let sp = SectionPos::new(i / 16, i % 16, 0);
                (i64::from(i), sp, col.clone())
            })
            .collect();

        world.admit_section_candidates(wanted.clone());
        assert_eq!(world.side.gen.pending_sections.len(), wanted.len());
        assert!(!world.side.gen.section_requests_unsettled);

        for job in world.side.gen.pending_section_jobs.values() {
            job.cancel();
        }
        release.send(()).unwrap();
    }

    #[test]
    fn a_failed_section_is_requested_again() {
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
        world.request_wanted_sections(target);
        let failed = *world
            .side
            .gen
            .pending_sections
            .iter()
            .next()
            .expect("a wanted section");
        if let Some(job) = world.side.gen.pending_section_jobs.remove(&failed) {
            job.cancel();
        }
        world.remove_pending_section(failed);
        world.side.gen.section_requests_unsettled = true;

        world.refill_section_requests();
        assert!(world.side.gen.pending_sections.contains(&failed));
        assert!(!world.side.gen.section_requests_unsettled);

        for job in world.side.gen.pending_section_jobs.values() {
            job.cancel();
        }
        release.send(()).unwrap();
    }
}
