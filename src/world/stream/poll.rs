use crate::world::ServerWorld;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

use crate::save::SectionRecord;
use crate::worker::{GenJob, GenOutput};
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use petramond_worldgen::ColumnGen;

use crate::world::store::{LoadTarget, SkyCoverChange};

use super::StreamEvent;

/// Drain finished worldgen by TIME with a count floor: installs are cheap (map
/// insert + classify), so a fixed count frame-quantized big bursts (a whole r=20
/// disc took ~100 frames just to drain at 128/frame), while the budget still keeps
/// one frame from installing an unbounded burst and starving rendering.
///
/// The server world's poll runs on the ~200 Hz server pump with no frame to
/// protect: a tighter 750 µs budget capped install throughput (~150 ms/s of
/// drain time) right at RD32 sprint-flight demand — the server's own loaded
/// set fell to half the wanted disc. Disk answers drain on the same budget.
const GEN_DRAIN_MIN_PER_POLL: usize = 16;
const DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(2_500);
const DISK_DRAIN_MIN_PER_POLL: usize = 16;

impl ServerWorld {
    fn install_column_gen(&mut self, pos: ChunkPos, col: Arc<ColumnGen>) {
        {
            let column = self.data.ensure_column(pos);
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    column.set_biome(x, z, col.biome_at(x, z));
                    let surface = col.heightmap_surface_y(x, z);
                    column.set_surface_y(x, z, surface);
                    column.set_sky_cover_y(x, z, surface);
                }
            }
        }
        if self
            .side
            .save
            .as_ref()
            .is_some_and(|s| !s.colgen_manifest_contains(pos))
        {
            self.side
                .gen
                .pending_colgen_records
                .push(col.cache_record(self.data.seed));
        }
        self.set_column_gen(pos, col);
        self.data.bump_column_payload_revision(pos);
        if self.data.last_load_target.is_some_and(|t| t.center == pos)
            || self.data.extra_load_targets.iter().any(|t| t.center == pos)
        {
            self.refresh_generation_priorities();
        }
    }

    fn slim_settled_column_gen(&mut self, pos: ChunkPos) {
        let Some(col) = self.side.gen.column_gen.get(&pos) else {
            return;
        };
        if !col.has_feature_windows() {
            return;
        }
        let slim = std::sync::Arc::new(col.slimmed());
        self.side.gen.column_gen.insert(pos, slim);
    }

    fn best_target_for_column(&self, target: LoadTarget, pos: ChunkPos) -> LoadTarget {
        let mut best = target;
        let mut best_key = target.column_priority_key(pos);
        for t in &self.data.extra_load_targets {
            let key = t.column_priority_key(pos);
            if key < best_key {
                best = *t;
                best_key = key;
            }
        }
        best
    }

    pub fn set_stream_event_capture(&mut self, on: bool) {
        if !on {
            self.side.stream_events.clear();
        }
        self.side.stream_events_enabled = on;
    }

    pub fn take_stream_events(&mut self) -> Vec<StreamEvent> {
        std::mem::take(&mut self.side.stream_events)
    }

    pub fn poll(&mut self) -> usize {
        let before = self.stream_finality_fingerprint();
        let new_columns = self.poll_inner();
        if self.stream_finality_fingerprint() != before {
            self.bump_terrain_revision();
        }
        new_columns
    }

    fn stream_finality_fingerprint(&self) -> (usize, usize, usize, usize) {
        (
            self.data.sections.len(),
            self.side.gen.pending_sections.len(),
            self.side.gen.awaited_overlays.len(),
            self.side.gen.pending_overlays.len(),
        )
    }

    fn drain_budgeted<T>(
        &mut self,
        min: usize,
        budget: std::time::Duration,
        mut pop: impl FnMut(&mut Self) -> Option<T>,
        mut apply: impl FnMut(&mut Self, T),
    ) {
        let start = std::time::Instant::now();
        let mut drained = 0usize;
        while drained < min || start.elapsed() < budget {
            let Some(item) = pop(self) else {
                break;
            };
            drained += 1;
            apply(self, item);
        }
    }

    fn poll_inner(&mut self) -> usize {
        let target = self
            .data
            .last_load_target
            .unwrap_or_else(|| LoadTarget::new(0, 0, 0, self.data.render_dist));
        let mut new_columns = 0usize;
        let mut new_column_positions: Vec<ChunkPos> = Vec::new();
        let mut ingested: Vec<SectionPos> = Vec::new();
        let mut ingested_set: FxHashSet<SectionPos> = FxHashSet::default();
        let mut heightmap_recompute: FxHashSet<ChunkPos> = FxHashSet::default();
        let mut gen_ingested: FxHashSet<SectionPos> = FxHashSet::default();

        self.drain_budgeted(
            GEN_DRAIN_MIN_PER_POLL,
            DRAIN_TIME_BUDGET,
            |w| w.side.worker.try_recv(),
            |w, out| match out {
                GenOutput::Column { pos, col } => {
                    let was_pending = w.side.gen.pending.remove(&pos).is_some();
                    if !was_pending {
                        return;
                    }
                    if !w.within_current_keep_radius(pos) {
                        return;
                    }
                    w.install_column_gen(pos, col);
                    new_columns += 1;
                    new_column_positions.push(pos);
                }
                GenOutput::ColumnFailed(pos) => {
                    w.side.gen.pending.remove(&pos);
                    w.data.missing_columns_settled = false;
                    w.data.deferred_recheck_needed = true;
                }
                GenOutput::SectionFailed(sp) => {
                    w.remove_pending_section(sp);
                    w.side.gen.pending_section_jobs.remove(&sp);
                    w.side.gen.section_requests_unsettled = true;
                    w.queue_deferred_rechecks_around(sp);
                }
                GenOutput::SectionDeferred { sp, col, pending } => {
                    if !w.side.gen.pending_section_jobs.contains_key(&sp)
                        || !w.within_current_keep_radius(sp.chunk_pos())
                    {
                        w.remove_pending_section(sp);
                        w.side.gen.pending_section_jobs.remove(&sp);
                        return;
                    }
                    let band_lo = *Self::surface_window_for_column(&col, 0).start();
                    let underground = w.anchor_underground(target);
                    let job = w.side.worker.submit(
                        target.deferred_section_key(sp, band_lo, underground),
                        GenJob::ResumeSection {
                            pending,
                            col,
                            seed: w.data.seed,
                        },
                    );
                    w.side.gen.pending_section_jobs.insert(sp, job);
                }
                GenOutput::Section { sp, section } => {
                    if !w.remove_pending_section(sp) {
                        return;
                    }
                    w.side.gen.pending_section_jobs.remove(&sp);
                    if !w.within_current_keep_radius(sp.chunk_pos())
                        || !w.side.gen.column_gen.contains_key(&sp.chunk_pos())
                    {
                        return;
                    }
                    w.data.sections.insert(sp, section);
                    w.note_section_loaded(sp);
                    w.refresh_block_entity_index(sp);
                    w.refresh_presented_index(sp);
                    if w.side.stream_events_enabled {
                        w.side.stream_events.push(StreamEvent::Generated(sp));
                    }
                    if ingested_set.insert(sp) {
                        ingested.push(sp);
                    }
                    gen_ingested.insert(sp);
                }
            },
        );

        // 1b. Column-gen cache ("Optimize explored terrain"). Hit just installs like a
        // generated column. Bad record or version mismatch: worker takes over, `pending`
        // stays so `GenOutput::Column` still catches it.
        self.drain_budgeted(
            DISK_DRAIN_MIN_PER_POLL,
            DRAIN_TIME_BUDGET,
            |w| {
                w.side
                    .save
                    .as_ref()
                    .and_then(|s| s.poll_loaded_column_gen())
            },
            |w, loaded| {
                let pos = loaded.pos;
                if !w.side.gen.pending.contains_key(&pos) {
                    return;
                }
                match loaded.record {
                    Some(rec) => {
                        w.side.gen.pending.remove(&pos);
                        if !w.within_current_keep_radius(pos) {
                            return;
                        }
                        let col = Arc::new(ColumnGen::from_cache_record(rec));
                        w.install_column_gen(pos, col);
                        new_columns += 1;
                        new_column_positions.push(pos);
                    }
                    None => {
                        if let Some(save) = w.side.save.as_mut() {
                            save.note_colgen_load_miss(pos);
                        }
                        let job = w.side.worker.submit(
                            target.column_priority_key(pos),
                            GenJob::Column {
                                pos,
                                seed: w.data.seed,
                            },
                        );
                        if let Some(slot) = w.side.gen.pending.get_mut(&pos) {
                            *slot = Some(job);
                        }
                    }
                }
            },
        );

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
        self.drain_budgeted(
            DISK_DRAIN_MIN_PER_POLL,
            DRAIN_TIME_BUDGET,
            |w| w.side.save.as_ref().and_then(|s| s.poll_loaded()),
            |w, loaded| {
                let sp = loaded.pos;
                let loaded_store = loaded.store;
                w.side.gen.awaited_overlays.remove(&sp);
                w.settle_stream_nonfinal(sp);
                let disk_primary = w.side.gen.disk_primary_sections.remove(&sp);
                if disk_primary {
                    w.remove_pending_section(sp);
                    w.side.gen.pending_section_jobs.remove(&sp);
                }
                if !w.within_current_keep_radius(sp.chunk_pos()) {
                    return;
                }
                let (section, entities, mobs) = match loaded.record {
                    SectionRecord::Decoded {
                        section,
                        entities,
                        mobs,
                    } => (*section, entities, mobs),
                    missing => {
                        if let Some(save) = w.side.save.as_mut() {
                            match &missing {
                                SectionRecord::Unreadable(unreadable) => save
                                    .note_section_unreadable(
                                        &mut w.data.saved,
                                        sp,
                                        loaded.store,
                                        unreadable,
                                    ),
                                _ => {
                                    save.note_section_load_miss(&mut w.data.saved, sp, loaded.store)
                                }
                            }
                        }
                        if disk_primary {
                            if let Some(col) = w.side.gen.column_gen.get(&sp.chunk_pos()).cloned() {
                                let band_lo = *Self::surface_window_for_column(&col, 0).start();
                                let underground = w.anchor_underground(target);
                                let job = w.side.worker.submit(
                                    target.surface_biased_section_key(sp, band_lo, underground),
                                    GenJob::Section {
                                        sp,
                                        col,
                                        seed: w.data.seed,
                                    },
                                );
                                w.insert_pending_section(sp);
                                w.side.gen.pending_section_jobs.insert(sp, job);
                            }
                        }
                        return;
                    }
                };
                if disk_primary {
                    if !w.side.gen.column_gen.contains_key(&sp.chunk_pos()) {
                        return;
                    }
                    if !entities.is_empty() || !mobs.is_empty() {
                        if let Some(save) = w.side.save.as_mut() {
                            save.note_record_holds_entities(sp);
                        }
                    }
                    w.data.sections.insert(sp, Arc::new(section));
                    w.note_section_loaded(sp);
                    w.refresh_block_entity_index(sp);
                    w.refresh_presented_index(sp);
                    w.side.entities.dropped_items.extend(entities);
                    w.restore_mobs(mobs);
                    if w.side.stream_events_enabled {
                        w.side.stream_events.push(StreamEvent::Loaded(sp));
                    }
                    if ingested_set.insert(sp) {
                        ingested.push(sp);
                    }
                    if loaded_store == crate::save::SectionStore::Authoritative {
                        heightmap_recompute.insert(sp.chunk_pos());
                    }
                } else {
                    w.side
                        .gen
                        .pending_overlays
                        .insert(sp, (section, entities, mobs));
                    w.note_stream_nonfinal(sp);
                }
            },
        );

        let overlaid = self.apply_pending_overlays();
        if self.side.stream_events_enabled {
            for sp in &overlaid {
                self.side.stream_events.push(StreamEvent::Loaded(*sp));
            }
        }
        for sp in &overlaid {
            if ingested_set.insert(*sp) {
                ingested.push(*sp);
            }
            heightmap_recompute.insert(sp.chunk_pos());
        }

        let ingested_columns: FxHashSet<ChunkPos> =
            ingested.iter().map(|sp| sp.chunk_pos()).collect();
        for pos in &ingested_columns {
            if !self.column_has_pending_section(*pos) {
                self.slim_settled_column_gen(*pos);
            }
        }
        if self.side.gen.pending_colgen_records.len() >= 128 {
            self.flush_pending_colgen_records();
        }

        if ingested.is_empty() {
            self.flush_settled_deferred_if_needed(target);
            if !self.data.missing_columns_settled && self.data.extra_load_targets.is_empty() {
                self.request_missing_columns(target);
            }
            self.refill_section_requests();
            return new_columns;
        }

        let mut sky_cover_changed: FxHashMap<ChunkPos, (SkyCoverChange, bool)> =
            FxHashMap::default();
        let note_change = |map: &mut FxHashMap<ChunkPos, (SkyCoverChange, bool)>,
                           cp: ChunkPos,
                           change: SkyCoverChange,
                           from_persist: bool| {
            map.entry(cp)
                .and_modify(|(all, fp)| {
                    all.merge(change);
                    *fp &= from_persist;
                })
                .or_insert((change, from_persist));
        };
        for &sp in &ingested {
            let cp = sp.chunk_pos();
            if !heightmap_recompute.contains(&cp) {
                if let Some(change) = self.data.raise_column_heightmaps_from_section(sp) {
                    if change.escapes_section_neighborhood(sp) {
                        note_change(
                            &mut sky_cover_changed,
                            cp,
                            change,
                            !gen_ingested.contains(&sp),
                        );
                    }
                }
            }
        }
        let gen_columns: FxHashSet<ChunkPos> =
            gen_ingested.iter().map(|sp| sp.chunk_pos()).collect();
        for cp in heightmap_recompute {
            if let Some(change) = self.recompute_column_heightmaps(cp) {
                note_change(
                    &mut sky_cover_changed,
                    cp,
                    change,
                    !gen_columns.contains(&cp),
                );
            }
        }
        self.mark_sky_cover_light_dirty_around_many(sky_cover_changed);
        self.grow_sky_caverns(&ingested, &ingested_columns, target);

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
            let no_mesh_output = self.section_produces_no_mesh(sp);
            let stale = light_stale.contains(&sp);
            if self.data.sections.contains_key(&sp) {
                if stale {
                    self.mark_light_dirty_pos(sp);
                }
                self.light_bakes.cancel(sp);
                let needs_bake = self
                    .data
                    .sections
                    .get(&sp)
                    .is_some_and(|s| s.light_dirty && !s.all_opaque());
                if !no_mesh_output || needs_bake {
                    self.data.light_deferred.insert(sp);
                    self.data.deferred_rechecks.insert(sp);
                }
            }
        }
        self.data.deferred_rechecks.extend(affected.iter().copied());
        self.flush_settled_deferred_if_needed(target);

        self.queue_loaded_section_fluid_updates(&ingested);
        if !self.data.missing_columns_settled && self.data.extra_load_targets.is_empty() {
            self.request_missing_columns(target);
        }
        self.refill_section_requests();
        new_columns
    }

    pub fn mob_census_loaded_around(&self, center: ChunkPos, radius: i32) -> bool {
        let radius = radius.max(0);
        let stream_radius = self.data.render_dist.max(0);
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dz * dz > stream_radius * stream_radius {
                    continue;
                }
                let pos = ChunkPos::new(center.cx + dx, center.cz + dz);
                if !self.data.columns.contains_key(&pos) {
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
        !self.side.gen.awaited_overlays.iter().any(in_neighborhood)
            && !self.side.gen.pending_overlays.keys().any(in_neighborhood)
    }

    pub(super) fn apply_pending_overlays(&mut self) -> Vec<SectionPos> {
        let ready: Vec<SectionPos> = self
            .side
            .gen
            .pending_overlays
            .keys()
            .copied()
            .filter(|sp| self.data.sections.contains_key(sp))
            .collect();
        for sp in &ready {
            let (section, entities, mobs) = self.side.gen.pending_overlays.remove(sp).unwrap();
            self.settle_stream_nonfinal(*sp);
            if !entities.is_empty() || !mobs.is_empty() {
                if let Some(save) = self.side.save.as_mut() {
                    save.note_record_holds_entities(*sp);
                }
            }
            self.data.sections.insert(*sp, Arc::new(section));
            self.note_section_loaded(*sp);
            self.refresh_block_entity_index(*sp);
            self.refresh_presented_index(*sp);
            self.side.entities.dropped_items.extend(entities);
            self.restore_mobs(mobs);
        }
        ready
    }
}
