use rustc_hash::FxHashSet;

use crate::world::store::{LoadTarget, World};
use petramond_world::chunk::ChunkPos;

impl World {
    fn generation_targets(&self) -> Vec<LoadTarget> {
        self.last_load_target
            .into_iter()
            .chain(self.extra_load_targets.iter().copied())
            .collect()
    }

    pub(super) fn refresh_generation_priorities(&self) {
        if self.gen.pending.is_empty() && self.gen.pending_section_jobs.is_empty() {
            return;
        }
        let targets = self.generation_targets();
        if targets.is_empty() {
            return;
        }
        let underground: Vec<_> = targets
            .iter()
            .map(|t| self.anchor_underground(*t))
            .collect();
        let columns = self.gen.pending.iter().filter_map(|(pos, job)| {
            let ticket = job.as_ref()?.ticket;
            let key = targets.iter().map(|t| t.column_priority_key(*pos)).min()?;
            Some((ticket, key))
        });
        let sections = self
            .gen
            .pending_section_jobs
            .iter()
            .filter_map(|(sp, job)| {
                let col = self.gen.column_gen.get(&sp.chunk_pos())?;
                let band_lo = *Self::surface_window_for_column(col, 0).start();
                let key = targets
                    .iter()
                    .zip(&underground)
                    .map(|(t, &u)| t.surface_biased_section_key(*sp, band_lo, u))
                    .min()?;
                Some((job.ticket, key))
            });
        self.worker.reprioritize(columns.chain(sections));
    }

    pub(super) fn reclaim_far_column_requests(&mut self) {
        use super::requests::MAX_PENDING_COLUMN_GEN_JOBS;

        if self.gen.pending.is_empty() {
            return;
        }
        let targets = self.generation_targets();
        let mut seen = FxHashSet::default();
        let mut candidates = Vec::new();
        for target in &targets {
            for dz in -target.render_dist..=target.render_dist {
                for dx in -target.render_dist..=target.render_dist {
                    let pos = ChunkPos::new(target.center.cx + dx, target.center.cz + dz);
                    if !Self::column_wanted(*target, pos)
                        || self.gen.column_gen.contains_key(&pos)
                        || !seen.insert(pos)
                    {
                        continue;
                    }
                    let key = targets
                        .iter()
                        .map(|t| t.column_priority_key(pos))
                        .min()
                        .unwrap();
                    candidates.push((key, pos));
                }
            }
        }
        if candidates.len() > MAX_PENDING_COLUMN_GEN_JOBS {
            candidates.select_nth_unstable_by_key(MAX_PENDING_COLUMN_GEN_JOBS, |(key, pos)| {
                (*key, pos.cz, pos.cx)
            });
            candidates.truncate(MAX_PENDING_COLUMN_GEN_JOBS);
        }
        let nearest: FxHashSet<_> = candidates.into_iter().map(|(_, pos)| pos).collect();
        let removed = self.worker.remove_queued(
            self.gen
                .pending
                .iter()
                .filter_map(|(pos, job)| (!nearest.contains(pos)).then_some(job.as_ref()?.ticket)),
        );
        // Removing a queue entry is atomic with starting it. Running jobs and
        // disk reads keep their pending slots until their results arrive.
        self.gen.pending.retain(|_, job| {
            job.as_ref()
                .is_none_or(|job| !removed.contains(&job.ticket))
        });
    }
}
