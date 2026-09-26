use crate::world::ServerWorld;
use rustc_hash::FxHashSet;

use crate::world::store::LoadTarget;
use petramond_world::chunk::ChunkPos;

impl ServerWorld {
    fn generation_targets(&self) -> Vec<LoadTarget> {
        self.data
            .last_load_target
            .into_iter()
            .chain(self.data.extra_load_targets.iter().copied())
            .collect()
    }

    pub(super) fn refresh_generation_priorities(&self) {
        if self.side.gen.pending.is_empty() && self.side.gen.pending_section_jobs.is_empty() {
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
        let columns = self.side.gen.pending.iter().filter_map(|(pos, job)| {
            let ticket = job.as_ref()?.ticket;
            let key = targets.iter().map(|t| t.column_priority_key(*pos)).min()?;
            Some((ticket, key))
        });
        let sections = self
            .side
            .gen
            .pending_section_jobs
            .iter()
            .filter_map(|(sp, job)| {
                let col = self.side.gen.column_gen.get(&sp.chunk_pos())?;
                let band_lo = *Self::surface_window_for_column(col, 0).start();
                let key = targets
                    .iter()
                    .zip(&underground)
                    .map(|(t, &u)| t.surface_biased_section_key(*sp, band_lo, u))
                    .min()?;
                Some((job.ticket, key))
            });
        self.side.worker.reprioritize(columns.chain(sections));
    }

    /// Discard queued section work that no current anchor will use. Only
    /// tickets actually removed from the pool release their pending slots;
    /// running jobs still report normally. Saved overlays keep their read
    /// handshake.
    pub(super) fn prune_stale_section_requests(&mut self) {
        let targets = self.generation_targets();
        let stale = self
            .side
            .gen
            .pending_section_jobs
            .iter()
            .filter_map(|(sp, job)| {
                if self.side.gen.awaited_overlays.contains(sp) {
                    return None;
                }
                let wanted = self
                    .side
                    .gen
                    .column_gen
                    .get(&sp.chunk_pos())
                    .is_some_and(|col| {
                        targets.iter().any(|target| {
                            Self::column_wanted(*target, sp.chunk_pos())
                                && self
                                    .wanted_section_cys_for_column(
                                        sp.chunk_pos(),
                                        col,
                                        target.center_cy,
                                        0,
                                    )
                                    .contains(&sp.cy)
                        })
                    });
                (!wanted).then_some((*sp, job.ticket))
            })
            .collect::<Vec<_>>();
        let removed = self
            .side
            .worker
            .remove_queued(stale.iter().map(|(_, ticket)| *ticket));
        for (sp, ticket) in stale {
            if removed.contains(&ticket) {
                self.side.gen.pending_section_jobs.remove(&sp);
                self.remove_pending_section(sp);
                self.side.gen.section_requests_unsettled = true;
            }
        }
    }

    pub(super) fn reclaim_far_column_requests(&mut self) {
        use super::requests::MAX_PENDING_COLUMN_GEN_JOBS;

        if self.side.gen.pending.is_empty() {
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
                        || self.side.gen.column_gen.contains_key(&pos)
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
        let removed =
            self.side.worker.remove_queued(
                self.side.gen.pending.iter().filter_map(|(pos, job)| {
                    (!nearest.contains(pos)).then_some(job.as_ref()?.ticket)
                }),
            );
        // Removing a queue entry is atomic with starting it. Running jobs and
        // disk reads keep their pending slots until their results arrive.
        self.side.gen.pending.retain(|_, job| {
            job.as_ref()
                .is_none_or(|job| !removed.contains(&job.ticket))
        });
    }
}
