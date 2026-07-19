use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::block::Block;
use crate::block_state::{LogAxis, SlabState, StairState};
use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME};
use crate::door::DoorState;
use crate::facing::Facing;
use crate::net::protocol::{BlockDelta, CellState, ColumnPayload, LightPayload, SectionPayload};
use crate::section::{Section, SectionSummary};
use crate::torch::TorchPlacement;
use crate::world::store::{LoadTarget, World, WorldRole};

use super::map_entries;

impl World {
    /// Install a column's replicated facts on a replica: biome + both height
    /// maps into the `Column`, and the per-cy summaries into `column_summaries`
    /// (the replica's absent-section answer — see `section_summary`). The
    /// wire maps are authoritative and are NOT recomputed from installed
    /// sections, which may only partially cover the column. Idempotent — the
    /// sender re-ships only when the column revision changes, including
    /// immediately before a section unload changes an absent summary.
    pub(crate) fn install_remote_column(&mut self, payload: ColumnPayload) {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote installs are the replica's ingest path"
        );
        let expected_sections = Self::column_section_range().count();
        if payload.biomes.0.len() != SECTION_SIZE * SECTION_SIZE
            || payload.mesh_biomes.0.len() != 20 * 20
            || payload.surface_heightmap.len() != SECTION_SIZE * SECTION_SIZE
            || payload.sky_cover.len() != SECTION_SIZE * SECTION_SIZE
            || payload.summaries.len() != expected_sections
        {
            return;
        }
        let pos = payload.pos;
        let col = self.ensure_column(pos);
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
        self.column_summaries.insert(pos, summaries);
        self.column_biome_halos.insert(pos, payload.mesh_biomes.0);
        self.column_deep_band_los.insert(pos, payload.deep_band_lo);
        // Sections normally land AFTER their column (the sender orders it so),
        // but the deep classification must not silently die if that ordering
        // ever regresses: re-classify anything already installed in this
        // column now that the band floor is known.
        for cy in Self::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if self.sections.contains_key(&sp) {
                self.classify_deep_on_install(sp);
            }
        }
        // The sender re-ships only on its own revision change; move the
        // replica's revision so surface consumers resample the column.
        self.bump_column_payload_revision(pos);
    }

    /// Install one replicated section on a replica, entering at the same
    /// post-ingest seam `poll()` uses for a landed section. Shipped baked
    /// light seeds the cache (no rebake); without it the section and its
    /// neighbourhood are marked for the replica's own bake. Malformed buffer
    /// lengths drop the payload (a byte-corrupting transport, never the local
    /// connection).
    #[cfg(test)]
    pub(crate) fn install_remote_section(&mut self, payload: SectionPayload) {
        if let Some(pos) = self.install_remote_section_deferred(payload) {
            self.finish_remote_install_batch(&[pos]);
        }
    }

    /// Install without invalidating meshes yet. The message pump batches the
    /// overlapping neighbourhoods from all sections it received this frame.
    pub(crate) fn install_remote_section_deferred(
        &mut self,
        payload: SectionPayload,
    ) -> Option<SectionPos> {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote installs are the replica's ingest path"
        );
        let pos = payload.pos;
        if !SectionPos::cy_in_range(pos.cy) || payload.blocks.0.len() != SECTION_VOLUME {
            return None;
        }
        if payload
            .water
            .as_ref()
            .is_some_and(|w| w.0.len() != SECTION_VOLUME)
        {
            return None;
        }
        let s = &payload.states;
        let cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>> = s
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
            payload.blocks.0,
            payload.water.map(|w| w.0),
            // No furnace machine state on a replica: burn/cook counters are sim
            // state (progress reaches clients through menu sync), and the lit
            // face is the block id (`furnace_lit` is its own row).
            HashMap::new(),
            HashMap::new(), // container slots replicate via menu sync
            map_entries(&s.entity_facings, Facing::from_u8),
            map_entries(&s.torches, TorchPlacement::from_u8),
            map_entries(&s.model_cells, |off| off),
            map_entries(&s.model_facings, Facing::from_u8),
            map_entries(&s.doors, DoorState::decode),
            map_entries(&s.stairs, StairState::decode),
            map_entries(&s.slabs, |[meta, a, b]| {
                SlabState::decode(meta, Block::from_id(a), Block::from_id(b))
            }),
            map_entries(&s.log_axes, LogAxis::from_u8),
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

        self.ensure_column(pos.chunk_pos());
        self.sections.insert(pos, Arc::new(section));
        self.note_section_loaded(pos);
        // Installed content may change the visible surface without moving its
        // height (a same-height block swap) — surface consumers gate on
        // the column revision, so it must move with every section install.
        self.bump_column_payload_revision(pos.chunk_pos());
        // The post-ingest seam, minus gen/save bookkeeping (none exists here).
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.classify_deep_on_install(pos);
        Some(pos)
    }

    /// Invalidate every loaded section touched by a replica install batch once.
    /// This prevents contiguous terrain bursts from repeatedly bumping revisions
    /// and invalidating jobs for the same 3x3x3 overlap.
    pub(crate) fn finish_remote_install_batch(&mut self, installed: &[SectionPos]) {
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
        }
        self.vis_dirty = true;
    }

    /// Apply a server light rebake on a replica — the exact seam a local bake
    /// result enters through (`pump_light_bakes`' drain), minus the dirty/
    /// revision handshake: the server is authoritative, the cubes always land.
    pub(crate) fn install_remote_light(&mut self, payload: LightPayload) {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote light is the replica's ingest path"
        );
        if payload.skylight.0.len() != SECTION_VOLUME
            || payload
                .blocklight
                .as_ref()
                .is_some_and(|b| b.0.len() != SECTION_VOLUME)
        {
            return;
        }
        let pos = payload.pos;
        let Some(s) = self.section_mut(pos) else {
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
                crate::chunk::SKY_FULL,
            );
            let blk = match &payload.blocklight {
                Some(b) => {
                    crate::world::light::cube_region_changes(s.blocklight_arc().as_deref(), &b.0, 0)
                }
                None => match s.blocklight_arc() {
                    Some(old) => crate::world::light::cube_region_changes(
                        Some(&old),
                        &crate::world::light::ZERO_CUBE,
                        0,
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
        self.bump_lighting_revision();
        self.dirty_meshes.push(pos);
        if !first_bake {
            self.requeue_meshes_sampling_changed_regions(pos, mask);
        }
    }

    /// Apply one authoritative server delta on a replica: write the cell
    /// unconditionally (no `stream_writable` gate — the server already
    /// arbitrated) and update everything RENDERING needs — counters/state
    /// clears via the section setters, column-map patch, light + mesh dirtying
    /// — but schedule NO sim work: no water checks, no block updates, no
    /// `modified` flag. Deltas for absent sections drop silently (the server
    /// only streams deltas for sections in the recipient's sent set; a race
    /// with an unload is benign).
    ///
    /// Per-cell state: the block write already wipes the cell's sparse maps
    /// (`clear_on_block_change`) and the chest/furnace front is retired
    /// explicitly, mirroring the server's break funnels — so `state: None`
    /// leaves the cell clean and `Some` re-installs exactly one entry.
    pub(crate) fn apply_remote_delta(&mut self, delta: BlockDelta) {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote deltas are the replica's ingest path"
        );
        let Some((pos, lx, ly, lz)) = Self::split_world(delta.pos.x, delta.pos.y, delta.pos.z)
        else {
            return;
        };
        if !self.sections.contains_key(&pos) {
            return;
        }
        {
            let section = self.section_mut(pos).expect("presence checked above");
            // The raw write clears the cell's sparse state + water meta — the
            // same wipe the server's own write performed. Water then rides on
            // top of the cleared cell.
            section.set_block_raw(lx, ly, lz, delta.block_id);
            if let Some(meta) = delta.water {
                section.set_water(lx, ly, lz, Block::from_id(delta.block_id), meta);
            }
            section.take_entity_facing(lx, ly, lz);
            match delta.state {
                None => {}
                Some(CellState::Door(b)) => {
                    section.set_door_state(lx, ly, lz, DoorState::decode(b))
                }
                Some(CellState::Stair(b)) => {
                    section.set_stair_state(lx, ly, lz, StairState::decode(b))
                }
                Some(CellState::Slab([meta, a, b])) => section.set_slab_state(
                    lx,
                    ly,
                    lz,
                    SlabState::decode(meta, Block::from_id(a), Block::from_id(b)),
                ),
                Some(CellState::LogAxis(b)) => {
                    section.set_log_axis(lx, ly, lz, LogAxis::from_u8(b))
                }
                Some(CellState::Torch(b)) => {
                    section.insert_torch(lx, ly, lz, TorchPlacement::from_u8(b))
                }
                Some(CellState::Facing(b)) => {
                    section.insert_entity_facing(lx, ly, lz, Facing::from_u8(b));
                }
                Some(CellState::ModelCell { off, facing }) => {
                    // The base cell's offset stays implicit (no [0,0,0] entry),
                    // mirroring `place_model_block_facing`.
                    if off != [0, 0, 0] {
                        section.set_model_offset(lx, ly, lz, off);
                    }
                    section.set_model_facing(lx, ly, lz, Facing::from_u8(facing));
                }
            }
            // The raw write flagged light dirty; on a replica light is
            // server-owned — keep sampling the old cubes until the server's
            // rebake of this neighbourhood lands as `LightData` (a pump or
            // two; the block itself appears immediately).
            section.mark_light_clean();
        }
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        // Heightmap keeps replica physics/sky queries truthful; the light it
        // would invalidate on a streaming world is server-owned here.
        let _ = self.update_column_heights_after_set(
            delta.pos.x,
            delta.pos.y,
            delta.pos.z,
            Block::from_id(delta.block_id),
        );
        // Geometry samplers only; light-driven remeshes ride the server's
        // `LightData` install for exactly the sections whose cubes changed.
        self.queue_dirty_meshes_sampling_cell(delta.pos.x, delta.pos.y, delta.pos.z);
        self.vis_dirty = true;
    }

    /// Replica-only: set the view centre that orders mesh/light work
    /// (nearest-first) and anchors the always-mesh near ring — the replica's
    /// stand-in for the load target a streaming world maintains. Pure
    /// prioritisation: no gen, save, or streaming bookkeeping is touched.
    pub(crate) fn set_replica_view_center(&mut self, cx: i32, cy: i32, cz: i32) {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "streaming worlds derive their view centre from update_load*"
        );
        let target = LoadTarget::new(cx, cy, cz, self.render_dist);
        if self.last_load_target != Some(target) {
            self.last_load_target = Some(target);
            self.vis_dirty = true;
        }
    }

    /// Drop one section from a replica on the server's `SectionUnload` — the
    /// keep-shape eviction mirror. Absent sections then answer physics from
    /// the column summaries again. Returns the evicted section so the game's
    /// section cache can park it; the store no longer holds the `Arc`, so
    /// later deltas/light for the pos can never mutate the parked copy.
    pub(crate) fn uninstall_remote_section(&mut self, pos: SectionPos) -> Option<Arc<Section>> {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote unloads are the replica's ingest path"
        );
        let evicted = self.sections.get(&pos).cloned();
        self.remove_section(pos);
        self.vis_dirty = true;
        evicted
    }

    /// Drop a whole column (all sections + column data + summaries) on the
    /// server's `ColumnUnload`. Returns the evicted live sections — a
    /// `ColumnUnload` implicitly drops them with no per-section message, so
    /// this is the section cache's only sight of them.
    pub(crate) fn uninstall_remote_column(
        &mut self,
        pos: ChunkPos,
    ) -> Vec<(SectionPos, Arc<Section>)> {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote unloads are the replica's ingest path"
        );
        let bits = self.section_column_cys.get(&pos).copied().unwrap_or(0);
        let mut evicted = Vec::with_capacity(bits.count_ones() as usize);
        Self::for_each_column_cy(bits, |cy| {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            if let Some(s) = self.sections.get(&sp) {
                evicted.push((sp, Arc::clone(s)));
            }
        });
        self.remove_column(pos);
        self.vis_dirty = true;
        evicted
    }

    /// Re-promote a cached evicted section on the server's `SectionCached` —
    /// the install seam of `install_remote_section_deferred` without the
    /// payload decode/reconstruction: the `Arc<Section>` still carries the
    /// exact counters, sparse maps, and final light it was evicted with. The
    /// caller batches the returned pos into `finish_remote_install_batch`
    /// like any other install.
    pub(crate) fn install_cached_section(
        &mut self,
        pos: SectionPos,
        section: Arc<Section>,
    ) -> SectionPos {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote installs are the replica's ingest path"
        );
        self.ensure_column(pos.chunk_pos());
        self.sections.insert(pos, section);
        self.note_section_loaded(pos);
        // Same rule as a full section install: newly visible surface content
        // must move the column revision for revision-gated surface sampling.
        self.bump_column_payload_revision(pos.chunk_pos());
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.classify_deep_on_install(pos);
        pos
    }
}
