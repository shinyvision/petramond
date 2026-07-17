use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use crate::worker::{GenJob, GenOutput};
use crate::worldgen::driver::ColumnGen;

use crate::world::store::{LoadTarget, SkyCoverChange, World, WorldRole};

use super::StreamEvent;

/// Drain finished worldgen by TIME with a count floor: installs are cheap (map
/// insert + classify), so a fixed count frame-quantized big bursts (a whole r=20
/// disc took ~100 frames just to drain at 128/frame), while the budget still keeps
/// one frame from installing an unbounded burst and starving rendering.
///
/// The budget is ROLE-aware: on a Combined world `poll` runs on the render
/// thread, so it stays tight; the headless server world's poll runs on the
/// ~200 Hz server pump with no frame to protect, and 750 µs there capped
/// install throughput (~150 ms/s of drain time) right at RD32 sprint-flight
/// demand — the server's own loaded set fell to half the wanted disc.
const GEN_DRAIN_MIN_PER_POLL: usize = 16;
const GEN_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(750);
const SERVER_GEN_DRAIN_TIME_BUDGET: std::time::Duration =
    std::time::Duration::from_micros(2_500);
const DISK_DRAIN_MIN_PER_POLL: usize = 16;
const DISK_DRAIN_TIME_BUDGET: std::time::Duration = std::time::Duration::from_micros(750);

impl World {
    fn gen_drain_time_budget(&self) -> std::time::Duration {
        match self.role {
            WorldRole::ServerHeadless => SERVER_GEN_DRAIN_TIME_BUDGET,
            _ => GEN_DRAIN_TIME_BUDGET,
        }
    }

    fn disk_drain_time_budget(&self) -> std::time::Duration {
        match self.role {
            WorldRole::ServerHeadless => SERVER_GEN_DRAIN_TIME_BUDGET,
            _ => DISK_DRAIN_TIME_BUDGET,
        }
    }
}

impl World {
    /// Install one column's shared gen data: set the per-column biome + an initial
    /// bare-ground surface and sky-cover maps, then keep the `Arc` for driving
    /// per-section jobs. Before features/player edits they are identical: the
    /// analytical top is solid ground or filtering water.
    fn install_column_gen(&mut self, pos: ChunkPos, col: Arc<ColumnGen>) {
        {
            let column = self.ensure_column(pos);
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    column.set_biome(x, z, col.biome_at(x, z));
                    // Submerged / floorless columns top out at the waterline; land cave
                    // mouths use their post-cave top so skylight can enter shafts.
                    let surface = col.heightmap_surface_y(x, z);
                    column.set_surface_y(x, z, surface);
                    column.set_sky_cover_y(x, z, surface);
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
    /// sections for column-map refresh + light + mesh. Returns the number of
    /// columns whose shared data was installed this call.
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

    /// Budgeted drain shared by `poll_inner`'s worker/save pumps: pop and apply
    /// results until the source runs dry, taking at least `min` per poll but
    /// stopping once `budget` elapses — a big burst (e.g. a vertical move that
    /// re-streams a whole disc layer) spreads its main-thread install/mark cost
    /// over a few frames instead of one giant spike, and the rest stays buffered
    /// at the source for the next poll.
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
        self.drain_budgeted(
            GEN_DRAIN_MIN_PER_POLL,
            self.gen_drain_time_budget(),
            |w| w.worker.try_recv(),
            |w, out| match out {
                GenOutput::Column { pos, col } => {
                    let was_pending = w.pending.remove(&pos).is_some();
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
                // A panicked gen job: clear the pending flag so the position can be
                // re-requested (or finally judged absent) instead of staying
                // in-flight forever — which would both hide the terrain and freeze
                // the sim guard around it.
                GenOutput::ColumnFailed(pos) => {
                    w.pending.remove(&pos);
                    // No longer pending and not installed: the column is
                    // missing again — let the scan re-find it.
                    w.missing_columns_settled = false;
                    w.deferred_recheck_needed = true;
                }
                GenOutput::SectionFailed(sp) => {
                    w.pending_sections.remove(&sp);
                    w.pending_section_jobs.remove(&sp);
                    w.queue_deferred_rechecks_around(sp);
                }
                GenOutput::Section { sp, section } => {
                    if !w.pending_sections.remove(&sp) {
                        return;
                    }
                    w.pending_section_jobs.remove(&sp);
                    if !w.within_current_keep_radius(sp.chunk_pos())
                        || !w.column_gen.contains_key(&sp.chunk_pos())
                    {
                        return;
                    }
                    w.sections.insert(sp, section);
                    w.refresh_block_entity_index(sp);
                    w.refresh_particle_emitter_index(sp);
                    w.classify_deep_on_install(sp);
                    if w.stream_events_enabled {
                        w.stream_events.push(StreamEvent::Generated(sp));
                    }
                    if ingested_set.insert(sp) {
                        ingested.push(sp);
                    }
                    gen_ingested.insert(sp);
                }
            },
        );

        // 1b. Column-gen cache answers ("Optimize explored terrain"): a hit
        //     installs exactly like a generated column; a miss (corrupt record,
        //     seed/version drift) hands the column to the worker — `pending`
        //     stays set so the existing `GenOutput::Column` arm resolves it.
        self.drain_budgeted(
            DISK_DRAIN_MIN_PER_POLL,
            self.disk_drain_time_budget(),
            |w| w.save.as_ref().and_then(|s| s.poll_loaded_column_gen()),
            |w, loaded| {
                let pos = loaded.pos;
                if !w.pending.contains_key(&pos) {
                    return;
                }
                match loaded.record {
                    Some(rec) => {
                        w.pending.remove(&pos);
                        if !w.within_current_keep_radius(pos) {
                            return;
                        }
                        let col = Arc::new(ColumnGen::from_cache_record(rec));
                        w.install_column_gen(pos, col);
                        new_columns += 1;
                        new_column_positions.push(pos);
                    }
                    None => {
                        if let Some(save) = w.save.as_mut() {
                            save.note_colgen_load_miss(pos);
                        }
                        let job = w.worker.submit(
                            target.column_priority_key(pos),
                            GenJob::Column { pos, seed: w.seed },
                        );
                        if let Some(slot) = w.pending.get_mut(&pos) {
                            *slot = Some(job);
                        }
                    }
                }
            },
        );

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
        self.drain_budgeted(
            DISK_DRAIN_MIN_PER_POLL,
            self.disk_drain_time_budget(),
            |w| w.save.as_ref().and_then(|s| s.poll_loaded()),
            |w, loaded| {
                let sp = loaded.pos;
                let loaded_store = loaded.store;
                // The save thread answered: the record is no longer in flight (whatever
                // the answer), so the sim guard must not keep the section blocked.
                w.awaited_overlays.remove(&sp);
                let disk_primary = w.disk_primary_sections.remove(&sp);
                if disk_primary {
                    w.pending_sections.remove(&sp);
                    w.pending_section_jobs.remove(&sp);
                }
                if !w.within_current_keep_radius(sp.chunk_pos()) {
                    return;
                }
                let Some(section) = loaded.section else {
                    if let Some(save) = w.save.as_mut() {
                        save.note_section_load_miss(sp, loaded.store);
                    }
                    // Missing/corrupt record. Overlay path: generation stands.
                    // Disk-primary path: no base exists — generate it after all.
                    if disk_primary {
                        if let Some(col) = w.column_gen.get(&sp.chunk_pos()).cloned() {
                            let band_lo = *Self::surface_window_for_column(&col, 0).start();
                            let underground = w.anchor_underground(target);
                            let job = w.worker.submit(
                                target.surface_biased_section_key(sp, band_lo, underground),
                                GenJob::Section {
                                    sp,
                                    col,
                                    seed: w.seed,
                                },
                            );
                            w.pending_sections.insert(sp);
                            w.pending_section_jobs.insert(sp, job);
                        }
                    }
                    return;
                };
                if disk_primary {
                    if !w.column_gen.contains_key(&sp.chunk_pos()) {
                        return; // column evicted while the read was in flight
                    }
                    if !loaded.entities.is_empty() || !loaded.mobs.is_empty() {
                        if let Some(save) = w.save.as_mut() {
                            save.note_record_holds_entities(sp);
                        }
                    }
                    w.sections.insert(sp, Arc::new(section));
                    w.refresh_block_entity_index(sp);
                    w.refresh_particle_emitter_index(sp);
                    w.classify_deep_on_install(sp);
                    w.dropped_items.extend(loaded.entities);
                    w.restore_mobs(loaded.mobs);
                    if w.stream_events_enabled {
                        w.stream_events.push(StreamEvent::Loaded(sp));
                    }
                    if ingested_set.insert(sp) {
                        ingested.push(sp);
                    }
                    if loaded_store == crate::save::SectionStore::Authoritative {
                        heightmap_recompute.insert(sp.chunk_pos());
                    }
                } else {
                    w.pending_overlays
                        .insert(sp, (section, loaded.entities, loaded.mobs));
                }
            },
        );

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

        // 5. Derived sections can only raise the analytical bare maps with
        // generated features. Authoritative records may have removed them, so
        // only those columns pay for a full vertical rescan. Cover changes that
        // escape normal ingest invalidation dirty their dependent vertical band:
        // the planner's Full/Dark shortcut depends on the 2D map, not just the
        // ingested section's 3×3×3.
        // Each change carries whether it was raised purely by PERSISTED record
        // content (disk-primary / overlay) — such changes spare sections still
        // holding their untouched persisted bake (see
        // `mark_sky_cover_light_dirty_around_many`), so reloading explored
        // terrain re-queues no bakes.
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
                if let Some(change) = self.raise_column_heightmaps_from_section(sp) {
                    // Step 6 already invalidates a generated section's 3x3x3.
                    // Normal terrain/tree cover moves fit that band; only a
                    // larger vertical jump needs this extra map-wide pass.
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
    pub(super) fn apply_pending_overlays(&mut self) -> Vec<SectionPos> {
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
}
