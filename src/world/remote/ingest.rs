use crate::world::ReplicaWorld;
use crate::world::cell_change::{CellChange, ChangeKind};
use crate::world::store::for_each_column_cy;
use crate::world::WorldData;
use std::collections::BTreeMap;
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::world::replication::{BlockDelta, ColumnPayload, LightPayload, SectionPayload};
use crate::world::store::LoadTarget;
use petramond_world::block::Block;
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME};
use petramond_world::section::{CellMap, Section, SectionSummary};

impl ReplicaWorld {
    /// Install a column's replicated facts on a replica: biome + both height
    /// maps into the `Column`, and the per-cy summaries into `column_summaries`
    /// (the replica's absent-section answer — see `section_summary`). The
    /// wire maps are authoritative and are NOT recomputed from installed
    /// sections, which may only partially cover the column. Idempotent — the
    /// sender re-ships only when the column revision changes, including
    /// immediately before a section unload changes an absent summary.
    pub fn install_remote_column(&mut self, payload: ColumnPayload) {
        let expected_sections = WorldData::column_section_range().count();
        if payload.biomes.0.len() != SECTION_SIZE * SECTION_SIZE
            || payload.mesh_biomes.0.len() != 20 * 20
            || payload.surface_heightmap.len() != SECTION_SIZE * SECTION_SIZE
            || payload.sky_cover.len() != SECTION_SIZE * SECTION_SIZE
            || payload.summaries.len() != expected_sections
        {
            return;
        }
        let pos = payload.pos;
        let col = self.data.ensure_column(pos);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let i = z * SECTION_SIZE + x;
                if let Some(&b) = payload.biomes.0.get(i) {
                    col.set_biome(x, z, b);
                }
                if let Some(&h) = payload.surface_heightmap.get(i) {
                    col.set_surface_y(x, z, h);
                }
                if let Some(&h) = payload.sky_cover.get(i) {
                    col.set_sky_cover_y(x, z, h);
                }
            }
        }
        let summaries: Box<[SectionSummary]> = payload
            .summaries
            .iter()
            .map(|&b| SectionSummary::from_u8(b))
            .collect();
        self.data.column_summaries.insert(pos, summaries);
        self.data.column_biome_halos.insert(pos, payload.mesh_biomes.0);
        self.data.column_deep_band_los.insert(pos, payload.deep_band_lo);
        // Sections normally land AFTER their column (the sender orders it so),
        // but the deep classification must not silently die if that ordering
        // ever regresses: re-classify anything already installed in this
        // column now that the band floor is known.
        for cy in WorldData::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if self.data.sections.contains_key(&sp) {
                self.classify_deep_on_install(sp);
            }
        }
        // The sender re-ships only on its own revision change; move the
        // replica's revision so surface consumers resample the column.
        self.data.bump_column_payload_revision(pos);
    }

    /// Install one replicated section on a replica, entering at the same
    /// post-ingest seam `poll()` uses for a landed section. Shipped baked
    /// light seeds the cache (no rebake); without it the section and its
    /// neighbourhood are marked for the replica's own bake. Malformed buffer
    /// lengths drop the payload (a byte-corrupting transport, never the local
    /// connection).
    #[cfg(test)]
    pub fn install_remote_section(&mut self, payload: SectionPayload) {
        if let Some(pos) = self.install_remote_section_deferred(payload) {
            self.finish_remote_install_batch(&[pos]);
        }
    }

    /// Install without invalidating meshes yet. The message pump batches the
    /// overlapping neighbourhoods from all sections it received this frame.
    pub fn install_remote_section_deferred(
        &mut self,
        payload: SectionPayload,
    ) -> Option<SectionPos> {
        let pos = payload.pos;
        if !SectionPos::cy_in_range(pos.cy) || payload.blocks.0.len() != SECTION_VOLUME {
            return None;
        }
        if payload
            .fluid
            .as_ref()
            .is_some_and(|w| w.0.len() != SECTION_VOLUME)
        {
            return None;
        }
        let s = &payload.states;
        // Before the section install, because that path takes `payload`.
        let draws: Vec<crate::world::replication::BlockDrawEntry> = s.draws.clone();
        let cell_kv: CellMap<BTreeMap<String, Vec<u8>>> = s
            .cell_kv
            .iter()
            .map(|(cell, entries)| (*cell, entries.iter().cloned().collect()))
            .collect();
        if !payload.metrics.valid() {
            return None;
        }
        let mut section = Section::from_replica(
            pos.cx,
            pos.cy,
            pos.cz,
            petramond_world::section::BlockCube::from_ids(&payload.blocks.0),
            payload.fluid.map(|w| w.0),
            // No furnace machine state on a replica: burn/cook counters are sim
            // state (progress reaches clients through menu sync), and the lit
            // face is the block id (`furnace_lit` is its own row).
            CellMap::new(),
            CellMap::new(), // container slots replicate via menu sync
            // The unified state list installs verbatim — the transport already
            // rewrote the id-masked bytes into this session's block ids.
            s.cell_states.iter().copied().collect(),
            cell_kv,
            payload.metrics,
        );
        let light_seeded = payload
            .skylight
            .as_ref()
            .is_some_and(|l| l.0.len() == SECTION_VOLUME);
        if light_seeded {
            section.set_skylight(payload.skylight.expect("checked above").0);
            if let Some(bl) = payload.blocklight.filter(|l| l.0.len() == SECTION_VOLUME) {
                section.set_blocklight(bl.0);
            }
        } else {
            // The ship gate (`section_light_final`) only lets a lightless
            // section through when it never bakes (fully opaque) — final
            // as-is. Authoritative rebakes arrive as `LightData`; local
            // prediction light never enters through this ingest seam.
            section.mark_light_clean();
        }

        self.data.ensure_column(pos.chunk_pos());
        self.data.sections.insert(pos, Arc::new(section));
        self.note_section_loaded(pos);
        // Installed content may change the visible surface without moving its
        // height (a same-height block swap) — surface consumers gate on
        // the column revision, so it must move with every section install.
        self.data.bump_column_payload_revision(pos.chunk_pos());
        // Retained mod drawings arrive WITH the section, so a machine placed
        // before this client joined is drawn on the frame it streams in.
        //
        // The payload is this section's WHOLE draw state, like its cell states
        // and its KV, so it REPLACES rather than merges: a re-install (a
        // corrective resend, a re-stream) would otherwise leave a set the
        // server has since cleared drawing on the replica forever.
        self.forget_block_draws_in_section(pos);
        for (cell, prims) in draws {
            let (lx, ly, lz) = petramond_world::chunk::section_local(cell as usize);
            let at = petramond_math::math::IVec3::new(
                pos.cx * 16 + lx as i32,
                pos.cy * 16 + ly as i32,
                pos.cz * 16 + lz as i32,
            );
            self.apply_remote_block_draw(at, prims);
        }
        // The post-ingest seam, minus gen/save bookkeeping (none exists here).
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.classify_deep_on_install(pos);
        Some(pos)
    }

    /// Invalidate every loaded section touched by a replica install batch once.
    /// This prevents contiguous terrain bursts from repeatedly bumping revisions
    /// and invalidating jobs for the same 3x3x3 overlap.
    pub fn finish_remote_install_batch(&mut self, installed: &[SectionPos]) {
        if installed.is_empty() {
            return;
        }
        let mut affected = FxHashSet::default();
        for pos in installed {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        affected.insert(SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz));
                    }
                }
            }
        }
        for pos in affected {
            self.queue_dirty_mesh(pos);
            self.defer_stream_mesh(pos);
        }
        self.side.terrain.vis_dirty = true;
    }

    /// Apply a server light rebake on a replica — the exact seam a local bake
    /// result enters through (`pump_light_bakes`' drain), minus the dirty/
    /// revision handshake: the server is authoritative, the cubes always land.
    pub fn install_remote_light(&mut self, payload: LightPayload) {
        if payload.skylight.0.len() != SECTION_VOLUME
            || payload
                .blocklight
                .as_ref()
                .is_some_and(|b| b.0.len() != SECTION_VOLUME)
        {
            return;
        }
        let pos = payload.pos;
        let Some(s) = self.data.section_mut(pos) else {
            return; // unloaded while the message was in flight
        };
        // Region-diff against the cached cubes: authoritative light that
        // matches the replica's current cubes (a predicted edit's bake the
        // server agreed with) publishes no remesh, and a change requeues
        // exactly the neighbours that sampled the changed cells.
        let first_bake = !s.has_baked_light();
        let mask = if first_bake {
            crate::world::light::REGION_ALL
        } else {
            let sky = crate::world::light::cube_region_changes(
                s.skylight_arc().as_deref(),
                &payload.skylight.0,
                petramond_world::chunk::SKY_FULL,
            );
            let blk = match &payload.blocklight {
                Some(b) => crate::world::light::cube_region_changes(
                    s.blocklight_arc().as_deref(),
                    &b.0,
                    petramond_world::light::LightRgb::ZERO,
                ),
                None => match s.blocklight_arc() {
                    Some(old) => crate::world::light::cube_region_changes(
                        Some(&old),
                        &crate::world::light::ZERO_CUBE,
                        petramond_world::light::LightRgb::ZERO,
                    ),
                    None => 0,
                },
            };
            sky | blk
        };
        // Install the server's cubes regardless — they are authoritative and
        // an identical install is a couple of `Arc` swaps.
        s.set_skylight(payload.skylight.0);
        match payload.blocklight {
            Some(b) => s.set_blocklight(b.0),
            None => s.clear_blocklight(),
        }
        if mask == 0 {
            return;
        }
        s.dirty = true;
        // An in-flight mesh snapshotted the old cubes: discard its result.
        s.mesh_revision = s.mesh_revision.wrapping_add(1);
        self.data.bump_lighting_revision();
        self.side.terrain.dirty_meshes.push(pos);
        if !first_bake {
            self.requeue_meshes_sampling_changed_regions(pos, mask);
        }
    }

    /// Apply one authoritative server delta on a replica: write the cell
    /// unconditionally (no `stream_writable` gate — the server already
    /// arbitrated) and update everything RENDERING needs — counters/state
    /// clears via the section setters, column-map patch, light + mesh dirtying
    /// — but schedule NO sim work: no fluid checks, no block updates, no
    /// `modified` flag. Deltas for absent sections drop silently (the server
    /// only streams deltas for sections in the recipient's sent set; a race
    /// with an unload is benign).
    ///
    /// Per-cell state: the block write already wipes the cell's unified state
    /// (`clear_on_block_change`), mirroring the server's own write — so
    /// `state: None` leaves the cell clean and `Some` re-installs the entry
    /// verbatim (the transport already rewrote its id-masked bytes).
    pub fn apply_remote_delta(&mut self, delta: BlockDelta) {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(delta.pos.x, delta.pos.y, delta.pos.z)
        else {
            return;
        };
        if !self.data.sections.contains_key(&pos) {
            return;
        }
        let old = {
            let section = self.data.section_mut(pos).expect("presence checked above");
            let old = section.block(lx, ly, lz);
            // The raw write clears the cell's sparse state + fluid meta — the
            // same wipe the server's own write performed. Fluid meta then rides on
            // top of the cleared cell.
            section.set_block_raw(lx, ly, lz, delta.block_id);
            if let Some(meta) = delta.fluid {
                section.set_fluid(lx, ly, lz, Block::from_id(delta.block_id), meta);
            }
            if let Some(state) = delta.state {
                section.set_cell_state(lx, ly, lz, state);
            }
            // Re-install the cell's mod KV — the raw write wiped it exactly
            // like a server-side write would, but this delta may be a
            // CORRECTIVE snapshot of an unchanged cell whose KV the server
            // still holds (the gray-dye bug): the delta carries the truth.
            for (key, value) in delta.cell_kv {
                section.cell_kv_set(lx, ly, lz, key, value);
            }
            // The raw write flagged light dirty; on a replica light is
            // server-owned — keep sampling the old cubes until the server's
            // rebake of this neighbourhood lands as `LightData` (a pump or
            // two; the block itself appears immediately).
            section.mark_light_clean();
            old
        };
        self.apply_cell_changes(&[CellChange::new(delta.pos, old, ChangeKind::Remote)]);
    }

    /// Replica-only: apply one live per-cell mod KV delta (the streamed twin
    /// of the server's `cell_kv_set`/`cell_kv_remove`). Applied AFTER the
    /// batch's block deltas — a block delta's raw write wiped the cell's KV
    /// exactly like the server's own write, so a same-tick
    /// write-block-then-KV sequence restores in order. A custom-shape cell is
    /// re-marked for the client bake pump: replicated KV is presentation
    /// state a render bake may derive from (a dye vat's fluid tint), so a
    /// value change must re-bake and remesh even though no block changed.
    pub fn apply_remote_cell_kv(&mut self, kv: crate::world::replication::CellKvDelta) {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(kv.pos.x, kv.pos.y, kv.pos.z) else {
            return;
        };
        let Some(section) = self.data.section_mut(pos) else {
            return;
        };
        let affects_mesh = petramond_world::block::kv_key_affects_mesh(&kv.key);
        match kv.value {
            Some(value) => section.cell_kv_set(lx, ly, lz, kv.key, value),
            None => {
                section.cell_kv_remove(lx, ly, lz, &kv.key);
            }
        }
        let block = Block::from_id(self.data.chunk_block(kv.pos.x, kv.pos.y, kv.pos.z));
        self.mark_custom_bake_edit(kv.pos.x, kv.pos.y, kv.pos.z, block);
        // Mesh-feeding presentation keys render through the ordinary mesher
        // for NON-custom shapes (a dyed wool cube), which the custom-bake
        // re-mark above does not cover — re-mesh the section directly.
        if affects_mesh && !block.is_custom_shape() {
            self.queue_dirty_meshes_sampling_cell(kv.pos.x, kv.pos.y, kv.pos.z);
        }
    }

    /// Replica-only: set the view centre that orders mesh/light work
    /// (nearest-first) and anchors the always-mesh near ring — the replica's
    /// stand-in for the load target a streaming world maintains. Pure
    /// prioritisation: no gen, save, or streaming bookkeeping is touched.
    pub fn set_replica_view_center(&mut self, cx: i32, cy: i32, cz: i32) {
        let target = LoadTarget::new(cx, cy, cz, self.data.render_dist);
        if self.data.last_load_target != Some(target) {
            self.data.last_load_target = Some(target);
            self.side.terrain.vis_dirty = true;
        }
    }

    /// Drop one section from a replica on the server's `SectionUnload` — the
    /// keep-shape eviction mirror. Absent sections then answer physics from
    /// the column summaries again. Returns the evicted section so the game's
    /// section cache can park it; the store no longer holds the `Arc`, so
    /// later deltas/light for the pos can never mutate the parked copy.
    pub fn uninstall_remote_section(&mut self, pos: SectionPos) -> Option<Arc<Section>> {
        let evicted = self.data.sections.get(&pos).cloned();
        self.remove_section(pos);
        self.side.terrain.vis_dirty = true;
        evicted
    }

    /// Drop a whole column (all sections + column data + summaries) on the
    /// server's `ColumnUnload`. Returns the evicted live sections — a
    /// `ColumnUnload` implicitly drops them with no per-section message, so
    /// this is the section cache's only sight of them.
    pub fn uninstall_remote_column(&mut self, pos: ChunkPos) -> Vec<(SectionPos, Arc<Section>)> {
        let bits = self.data.section_column_cys.get(&pos).copied().unwrap_or(0);
        let mut evicted = Vec::with_capacity(bits.count_ones() as usize);
        for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if let Some(s) = self.data.sections.get(&sp) {
                evicted.push((sp, Arc::clone(s)));
            }
        });
        self.remove_column(pos);
        self.side.terrain.vis_dirty = true;
        evicted
    }

    /// Re-promote a cached evicted section on the server's `SectionCached` —
    /// the install seam of `install_remote_section_deferred` without the
    /// payload decode/reconstruction: the `Arc<Section>` still carries the
    /// exact counters, sparse maps, and final light it was evicted with. The
    /// caller batches the returned pos into `finish_remote_install_batch`
    /// like any other install.
    pub fn install_cached_section(&mut self, pos: SectionPos, section: Arc<Section>) -> SectionPos {
        self.data.ensure_column(pos.chunk_pos());
        self.data.sections.insert(pos, section);
        self.note_section_loaded(pos);
        // Same rule as a full section install: newly visible surface content
        // must move the column revision for revision-gated surface sampling.
        self.data.bump_column_payload_revision(pos.chunk_pos());
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.classify_deep_on_install(pos);
        pos
    }
}
