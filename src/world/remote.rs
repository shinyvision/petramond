//! World replication: the server-side payload builders and the client-side
//! replica install path (Phase B2).
//!
//! The server serializes nothing here — payloads carry `Arc` handles to the
//! live section buffers ([`SectionBytes`]), so the in-process connection ships
//! refcount bumps and the TCP transport does the encoding on its own threads.
//! Per-cell block STATE rides in [`SectionStatesPayload`] using the save
//! codec's exact per-entry encodings (`DoorState::encode`, `Facing::to_u8`, …)
//! so replication is as lossless as a save/load roundtrip.
//!
//! The replica ([`WorldRole::ClientReplica`]) never generates, ticks, or
//! saves: installs enter at the same post-ingest seam `poll()` uses for a
//! landed section (block-entity index, particle-emitter index, deep
//! classification, light + mesh queueing) but touch NO gen bookkeeping, save
//! bookkeeping, or `sim_guard` sets — on a replica those sets stay empty, so
//! the streaming-finality guard is structurally idle. For ABSENT sections the
//! replica answers physics/placement queries from the `ColumnPayload`
//! summaries (`World::column_summaries`), mirroring how `column_gen` answers
//! for the combined world.
//!
//! Deliberately absent from section payloads (they replicate elsewhere,
//! Phase C/F): container slot contents, furnace machine counters (only the
//! lit face ships — the replica installs a minimal lit stand-in so the mesher
//! renders it), mobs, and dropped items.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::block::Block;
use crate::block_state::{LogAxis, SlabState, StairState};
use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME};
use crate::door::DoorState;
use crate::facing::Facing;
use crate::furnace::Furnace;
use crate::net::protocol::{
    BlockDelta, CellState, ColumnPayload, LightPayload, SectionBytes, SectionPayload,
    SectionStatesPayload,
};
use crate::section::{Section, SectionSummary};
use crate::torch::TorchPlacement;

use super::store::{LoadAnchor, LoadTarget, World, WorldRole};

/// Sparse map → sorted wire entries, so identical state encodes identically
/// (the same reproducibility rule as the save codec's `put_indexed`).
fn sorted_entries<T, U>(map: &HashMap<u16, T>, mut f: impl FnMut(&T) -> U) -> Vec<(u16, U)> {
    let mut out: Vec<(u16, U)> = map.iter().map(|(&cell, v)| (cell, f(v))).collect();
    out.sort_unstable_by_key(|(cell, _)| *cell);
    out
}

/// Wire entries → sparse map (the install-side inverse of [`sorted_entries`]).
fn map_entries<T, U: Copy>(entries: &[(u16, U)], mut f: impl FnMut(U) -> T) -> HashMap<u16, T> {
    entries.iter().map(|&(cell, v)| (cell, f(v))).collect()
}

impl Section {
    /// Snapshot this section as its wire payload: `Arc` refcount bumps for the
    /// block/water/light buffers (no copies) plus the sparse state maps,
    /// encoded losslessly. Baked light rides along on EVERY transport — the
    /// ship gate (`section_light_final`) guarantees it is present unless the
    /// section never bakes (fully opaque); the replica does no light work.
    pub(crate) fn to_payload(&self) -> SectionPayload {
        let mut furnaces_lit: Vec<u16> = self
            .furnaces()
            .iter()
            .filter(|(_, f)| f.is_lit())
            .map(|(&cell, _)| cell)
            .collect();
        furnaces_lit.sort_unstable();
        let mut cell_kv: Vec<crate::net::protocol::CellKvEntry> = self
            .cell_kv()
            .iter()
            .map(|(&cell, map)| {
                // BTreeMap iteration is key-sorted: deterministic on the wire.
                let entries = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                (cell, entries)
            })
            .collect();
        cell_kv.sort_unstable_by_key(|(cell, _)| *cell);

        SectionPayload {
            pos: SectionPos::new(self.cx, self.cy, self.cz),
            blocks: SectionBytes(self.blocks_arc()),
            metrics: self.stream_metrics(),
            water: self.water_arc().map(SectionBytes),
            skylight: self.skylight_arc().map(SectionBytes),
            blocklight: self.blocklight_arc().map(SectionBytes),
            states: SectionStatesPayload {
                doors: sorted_entries(self.doors(), |s| s.encode()),
                stairs: sorted_entries(self.stair_states(), |s| s.encode()),
                slabs: sorted_entries(self.slab_states(), |s| {
                    [s.encode_meta(), s.layers[0].0, s.layers[1].0]
                }),
                log_axes: sorted_entries(self.log_axes(), |a| a.to_u8()),
                torches: sorted_entries(self.torches(), |t| t.to_u8()),
                saplings: sorted_entries(self.sapling_stages(), |&s| s),
                entity_facings: sorted_entries(self.entity_facings(), |f| f.to_u8()),
                model_facings: sorted_entries(self.model_facings(), |f| f.to_u8()),
                model_cells: sorted_entries(self.model_cells(), |&off| off),
                furnaces_lit,
                cell_kv,
            },
        }
    }
}

impl World {
    /// One column's client-relevant facts: biome skin, surface heightmap, and
    /// a per-cy [`SectionSummary`] for the whole world height range so replica
    /// physics can answer for absent sections. `None` for an unloaded column.
    pub(crate) fn column_payload(&self, pos: ChunkPos) -> Option<ColumnPayload> {
        let col = self.columns.get(&pos)?;
        let mut biomes = vec![0u8; SECTION_SIZE * SECTION_SIZE];
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                biomes[z * SECTION_SIZE + x] = col.biome_at(x, z);
            }
        }
        let summaries = Self::column_section_range()
            .map(|cy| {
                self.section_summary(SectionPos::new(pos.cx, cy, pos.cz))
                    .to_u8()
            })
            .collect();
        let (mesh_biomes, deep_band_lo) = self.column_gen.get(&pos).map_or_else(
            || {
                let mut halo = vec![0u8; 20 * 20];
                for z in 0..20 {
                    for x in 0..20 {
                        halo[z * 20 + x] =
                            col.biome_at(x.saturating_sub(2).min(15), z.saturating_sub(2).min(15));
                    }
                }
                (
                    Arc::from(halo.into_boxed_slice()),
                    crate::chunk::SECTION_MIN_CY,
                )
            },
            |gen| {
                (
                    gen.mesh_biome(),
                    *Self::surface_window_for_column(gen, 0).start(),
                )
            },
        );
        Some(ColumnPayload {
            pos,
            biomes: SectionBytes(Arc::from(biomes.into_boxed_slice())),
            mesh_biomes: SectionBytes(mesh_biomes),
            heightmap: col.heightmap_slice().to_vec(),
            summaries,
            deep_band_lo,
        })
    }

    /// Install a column's replicated facts on a replica: biome + heightmap
    /// into the `Column`, and the per-cy summaries into `column_summaries`
    /// (the replica's absent-section answer — see `section_summary`). The
    /// wire heightmap is the server's authoritative surface and is NOT
    /// recomputed from installed sections, which may only partially cover the
    /// column. Idempotent — the sender re-ships only when the column revision
    /// changes, including immediately before a section unload changes an absent
    /// summary.
    pub(crate) fn install_remote_column(&mut self, payload: ColumnPayload) {
        debug_assert!(
            self.role == WorldRole::ClientReplica,
            "remote installs are the replica's ingest path"
        );
        let expected_sections = Self::column_section_range().count();
        if payload.biomes.0.len() != SECTION_SIZE * SECTION_SIZE
            || payload.mesh_biomes.0.len() != 20 * 20
            || payload.heightmap.len() != SECTION_SIZE * SECTION_SIZE
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
                if let Some(&h) = payload.heightmap.get(i) {
                    col.set_surface_y(x, z, h);
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
        // Lit furnaces install a minimal lit stand-in: the mesher keys the lit
        // face off `Furnace::is_lit`; the real counters are sim state and stay
        // server-side (progress reaches clients through menu sync, Phase C).
        let furnaces: HashMap<u16, Furnace> = s
            .furnaces_lit
            .iter()
            .map(|&cell| {
                (
                    cell,
                    Furnace {
                        cook_progress: 0,
                        burn_remaining: 1,
                        burn_max: 1,
                    },
                )
            })
            .collect();
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
            furnaces,
            HashMap::new(), // container slots replicate via menu sync (Phase C)
            map_entries(&s.entity_facings, Facing::from_u8),
            map_entries(&s.torches, TorchPlacement::from_u8),
            map_entries(&s.model_cells, |off| off),
            map_entries(&s.model_facings, Facing::from_u8),
            map_entries(&s.saplings, |v| v),
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
            // as-is. The replica NEVER bakes: rebakes arrive as `LightData`.
            section.mark_light_clean();
        }

        self.ensure_column(pos.chunk_pos());
        self.sections.insert(pos, Arc::new(section));
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
        s.set_skylight(payload.skylight.0);
        match payload.blocklight {
            Some(b) => s.set_blocklight(b.0),
            None => s.clear_blocklight(),
        }
        s.dirty = true;
        // An in-flight mesh snapshotted the old cubes: discard its result.
        s.mesh_revision = s.mesh_revision.wrapping_add(1);
        self.bump_lighting_revision();
        self.dirty_meshes.push(pos);
    }

    /// Apply one authoritative server delta on a replica: write the cell
    /// unconditionally (no `stream_writable` gate — the server already
    /// arbitrated) and update everything RENDERING needs — counters/state
    /// clears via the section setters, heightmap patch, light + mesh dirtying
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
            section.take_furnace(lx, ly, lz);
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
                    section.insert_entity_facing(lx, ly, lz, Facing::from_u8(b & 0x7F));
                    // The high bit carries the furnace lit state so the front
                    // texture flips without a full section payload. The
                    // stand-in never ticks on a replica (sim is server-owned),
                    // so `burn_remaining: 1` simply means "render lit" until
                    // the next delta or payload says otherwise.
                    if Block::from_id(delta.block_id).interaction()
                        == crate::block::BlockInteraction::OpenFurnace
                    {
                        section.insert_furnace(
                            lx,
                            ly,
                            lz,
                            Furnace {
                                burn_remaining: u16::from(b & 0x80 != 0),
                                ..Furnace::default()
                            },
                        );
                    }
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
        let _ = self.update_column_height_after_set(
            delta.pos.x,
            delta.pos.y,
            delta.pos.z,
            delta.block_id != Block::Air.id(),
        );
        self.mark_dirty_neighborhood(pos, true);
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
        let evicted = Self::column_section_range()
            .filter_map(|cy| {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                self.sections.get(&sp).map(|s| (sp, Arc::clone(s)))
            })
            .collect();
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
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.classify_deep_on_install(pos);
        pos
    }

    /// One loaded section's wire payload, or `None` when it isn't loaded.
    pub(crate) fn section_payload(&self, pos: SectionPos) -> Option<SectionPayload> {
        self.sections.get(&pos).map(|s| s.to_payload())
    }

    /// Whether `sp`'s light is presentable: baked (possibly stale — a pending
    /// rebake follows as `LightData`) or fully opaque (never bakes; neighbour
    /// meshes cull against it and sample nothing). The terrain sender holds a
    /// section back until this holds, so every install lands light-complete
    /// and the replica performs NO light work of its own.
    pub(crate) fn section_light_final(&self, sp: SectionPos) -> bool {
        self.sections
            .get(&sp)
            .is_some_and(|s| s.has_baked_light() || s.all_opaque())
    }

    /// Drain the sections whose server bake landed since the last streaming
    /// pump (ServerHeadless fills it in `pump_light_bakes`).
    pub(crate) fn take_light_ship_log(&mut self) -> Vec<SectionPos> {
        self.light_ship_log.drain().collect()
    }

    /// One section's CURRENT light cubes as a wire payload; `None` when the
    /// section is gone (an eviction race) or has never baked.
    pub(crate) fn light_payload(&self, pos: SectionPos) -> Option<LightPayload> {
        let s = self.sections.get(&pos)?;
        Some(LightPayload {
            pos,
            skylight: SectionBytes(s.skylight_arc()?),
            blocklight: s.blocklight_arc().map(SectionBytes),
        })
    }

    /// Opaque key over everything the per-connection wanted-vs-sent diff
    /// depends on: the anchor's load target (chunk/section centre and render
    /// distance) and the world's terrain-content revision.
    /// While the key is unchanged, a rescan cannot find new work — the sender
    /// skips it (mirroring how `update_load_target` gates its scans).
    /// The wanted-terrain shape for one connection: its anchor at the
    /// anchor's own radius (the connection's view distance), clamped by this
    /// world's `render_dist` budget.
    fn send_target(&self, anchor: LoadAnchor) -> LoadTarget {
        LoadTarget::new(
            anchor.cx,
            anchor.cy,
            anchor.cz,
            anchor.radius.clamp(1, self.render_dist),
        )
    }

    pub(crate) fn terrain_send_key(&self, anchor: LoadAnchor) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        (self.terrain_target_key(anchor), self.terrain_revision).hash(&mut h);
        h.finish()
    }

    /// Anchor-only part of [`terrain_send_key`](Self::terrain_send_key). A
    /// connection consumes its current plan across content revisions, but an
    /// anchor move invalidates that plan immediately.
    pub(crate) fn terrain_target_key(&self, anchor: LoadAnchor) -> u64 {
        let t = self.send_target(anchor);
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        (t.center.cx, t.center.cz, t.center_cy, t.render_dist).hash(&mut h);
        h.finish()
    }

    /// Diff one connection's WANTED terrain shape against what it was already
    /// sent: which loaded, stream-final sections to ship now (nearest-first,
    /// budgeted), and which sent sections/columns left the keep shape (or the
    /// server) and must unload client-side. Pure planning — the caller owns
    /// the sent sets and the message emission (column before its sections).
    ///
    /// The wanted/keep shapes are exactly the streamer's own
    /// (`column_wanted`/`column_kept` over the anchor's target), so a
    /// client is offered precisely what the server streams for its anchor.
    pub(crate) fn plan_terrain_send(
        &self,
        anchor: LoadAnchor,
        sent_columns: &FxHashSet<ChunkPos>,
        sent_sections: &FxHashSet<SectionPos>,
        budget: usize,
    ) -> TerrainSendPlan {
        let target = self.send_target(anchor);

        let mut sections: Vec<(i64, SectionPos)> = self
            .sections
            .keys()
            .filter(|sp| !sent_sections.contains(sp))
            .filter(|sp| Self::column_wanted(target, sp.chunk_pos()))
            .filter(|sp| self.stream_writable(**sp))
            .filter(|sp| self.section_light_final(**sp))
            .map(|&sp| (target.section_priority_key(sp), sp))
            .collect();
        sections.sort_unstable_by_key(|(key, _)| *key);
        sections.truncate(budget);
        let sections: Vec<SectionPos> = sections.into_iter().map(|(_, sp)| sp).collect();

        // Keep test mirrors `unload_far`'s column hysteresis; a section the
        // server itself evicted (vertical window exit) is gone from `sections`
        // and unloads client-side through the same message.
        let drop_columns: Vec<ChunkPos> = sent_columns
            .iter()
            .filter(|cp| !Self::column_kept(target, **cp) || !self.columns.contains_key(cp))
            .copied()
            .collect();
        let dropped_cols: FxHashSet<ChunkPos> = drop_columns.iter().copied().collect();
        let drop_sections: Vec<SectionPos> = sent_sections
            .iter()
            .filter(|sp| !dropped_cols.contains(&sp.chunk_pos()))
            .filter(|sp| {
                !Self::column_kept(target, sp.chunk_pos()) || !self.sections.contains_key(sp)
            })
            .copied()
            .collect();

        TerrainSendPlan {
            sections,
            drop_sections,
            drop_columns,
        }
    }
}

/// Output of [`World::plan_terrain_send`].
pub(crate) struct TerrainSendPlan {
    /// Loaded, stream-final, wanted, unsent sections — nearest-first, budgeted.
    pub(crate) sections: Vec<SectionPos>,
    /// Sent sections that left the keep shape or the server world.
    pub(crate) drop_sections: Vec<SectionPos>,
    /// Sent columns that left the keep shape (their sections drop with them).
    pub(crate) drop_columns: Vec<ChunkPos>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::block::Block;
    use crate::block_state::{LogAxis, SlabSplit, StairHalf, StairState};
    use crate::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ};
    use crate::facing::Facing;
    use crate::mathh::IVec3;
    use crate::net::protocol::BlockDelta;
    use crate::section::Section;
    use crate::slab::SlabSlot;
    use crate::torch::TorchPlacement;
    use crate::worker::JobPool;
    use crate::world::store::{LoadTarget, World, WorldRole};

    /// A flat-floored source world (Combined runs the same content paths the
    /// headless server will) and a fresh replica, sharing ONE job pool — the
    /// Phase C in-process topology.
    fn server_and_replica() -> (World, World) {
        let pool = Arc::new(JobPool::new(2));
        let mut server = World::new_with_pool(0, 1, WorldRole::Combined, pool.clone());
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        c.set_block(x, 64, z, Block::Stone);
                    }
                }
                server.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
            }
        }
        let replica = World::new_with_pool(0, 1, WorldRole::ClientReplica, pool);
        (server, replica)
    }

    /// The Phase B2 convergence contract: everything a client can SEE —
    /// A furnace lighting (or going out) after join must flip the replica's
    /// front texture: the lit state rides the facing delta's high bit — a
    /// full section payload only ships at join/stream time.
    #[test]
    fn furnace_lit_flip_reaches_the_replica_through_a_delta() {
        let (mut server, mut replica) = server_and_replica();
        let pos = IVec3::new(4, 65, 4);
        assert!(server.set_block_world(pos.x, pos.y, pos.z, Block::Furnace));
        server.insert_furnace(pos, Facing::South);

        for cp in server.columns.keys().copied().collect::<Vec<_>>() {
            replica.install_remote_column(server.column_payload(cp).unwrap());
        }
        for s in server.sections.values() {
            replica.install_remote_section(s.to_payload());
        }
        let replica_lit = |r: &World| {
            r.section_at_world_for_test(4, 65, 4)
                .expect("furnace section")
                .is_furnace_lit(4, 1, 4)
        };
        assert!(!replica_lit(&replica), "fixture: joins unlit");

        // The furnace lights; `tick_furnaces` announces flips through
        // `notify_block_and_neighbors`, which records the delta.
        server.set_replication_capture(true);
        {
            let (furnace, _) = server.furnace_parts_mut(pos).unwrap();
            furnace.burn_remaining = 50;
            furnace.burn_max = 100;
        }
        server.notify_block_and_neighbors(pos.x, pos.y, pos.z);
        for d in server.take_block_deltas() {
            replica.apply_remote_delta(d);
        }
        assert!(
            replica_lit(&replica),
            "the lit flip must reach the replica's mesher"
        );

        // ...and the flame going out flips it back.
        {
            let (furnace, _) = server.furnace_parts_mut(pos).unwrap();
            furnace.burn_remaining = 0;
        }
        server.notify_block_and_neighbors(pos.x, pos.y, pos.z);
        for d in server.take_block_deltas() {
            replica.apply_remote_delta(d);
        }
        assert!(!replica_lit(&replica), "the extinguish must reach it too");
    }

    /// block ids, water meta, and every sparse state map — reads back
    /// identically through the public query surface after installing the
    /// column payloads, the section payloads, and a tick's coalesced deltas.
    #[test]
    fn replica_converges_on_payloads_and_deltas() {
        let (mut server, mut replica) = server_and_replica();

        // One of each replicated state, through the normal edit funnels.
        assert!(server.set_block_world(2, 65, 2, Block::Stone));
        assert!(server.cell_kv_set(2, 65, 2, "testmod:heat".into(), vec![7, 1]));
        assert!(server.set_water_world(IVec3::new(3, 65, 3), Block::Water, 0)); // source
        assert!(server.set_water_world(IVec3::new(4, 65, 3), Block::Water, 0x83)); // falling
        assert!(server.place_door(IVec3::new(5, 65, 5), Block::OakDoor, Facing::East));
        assert!(server.place_stair(
            IVec3::new(6, 65, 6),
            Block::OakStairs,
            StairState::new(Facing::South, StairHalf::Top),
        ));
        assert!(server.set_block_world(7, 65, 7, Block::Torch));
        server.insert_torch(IVec3::new(7, 65, 7), TorchPlacement::East);
        assert!(server.place_log(IVec3::new(1, 65, 6), Block::OakLog, LogAxis::X));
        assert!(server.set_block_world(2, 65, 6, Block::OakSapling));
        server
            .section_at_world_mut_for_test(2, 65, 6)
            .unwrap()
            .set_sapling_stage(2, 1, 6, 2);
        assert!(server.set_block_world(1, 65, 1, Block::Chest));
        server.insert_chest(IVec3::new(1, 65, 1), Facing::West);
        assert!(server.set_block_world(4, 65, 4, Block::Furnace));
        server.insert_furnace(IVec3::new(4, 65, 4), Facing::South);
        {
            let (furnace, _) = server.furnace_parts_mut(IVec3::new(4, 65, 4)).unwrap();
            furnace.burn_remaining = 50;
            furnace.burn_max = 100;
        }
        assert!(server.place_slab_layer(
            IVec3::new(6, 65, 1),
            Block::CobblestoneSlab,
            SlabSlot {
                split: SlabSplit::Y,
                index: 0,
            },
        ));
        assert!(server.place_model_block_facing(
            IVec3::new(10, 65, 10),
            Block::FurnitureWorkbench,
            Facing::East,
        ));

        // Join-time capture: columns + sections. One deep all-stone section is
        // deliberately withheld so the summaries have to answer for it.
        let held_back = SectionPos::new(0, -2, 0);
        assert!(
            server.sections.contains_key(&held_back),
            "fixture: deep stone loaded"
        );
        let columns: Vec<_> = server
            .columns
            .keys()
            .copied()
            .map(|cp| server.column_payload(cp).expect("column loaded"))
            .collect();
        let sections: Vec<_> = server
            .sections
            .iter()
            .filter(|(sp, _)| **sp != held_back)
            .map(|(_, s)| s.to_payload())
            .collect();

        // Post-join edits ride the delta log.
        server.set_replication_capture(true);
        assert!(server.set_block_world(8, 65, 8, Block::Dirt));
        assert!(server.set_water_world(IVec3::new(9, 65, 9), Block::Water, 0x05));
        let deltas = server.take_block_deltas();
        assert!(!deltas.is_empty());

        for c in columns {
            replica.install_remote_column(c);
        }
        for s in sections {
            replica.install_remote_section(s);
        }
        for d in &deltas {
            replica.apply_remote_delta(*d);
        }
        // A delta for a section nobody installed drops silently.
        replica.apply_remote_delta(BlockDelta {
            pos: IVec3::new(200, 65, 200),
            block_id: Block::Stone.id(),
            water: None,
            state: None,
        });
        assert_eq!(replica.chunk_block(200, 65, 200), 0);

        // Raw content converges (blocks + water meta) at every touched cell.
        for (x, y, z) in [
            (2, 65, 2),
            (3, 65, 3),
            (4, 65, 3),
            (5, 65, 5),
            (5, 66, 5),
            (6, 65, 6),
            (7, 65, 7),
            (1, 65, 6),
            (2, 65, 6),
            (1, 65, 1),
            (4, 65, 4),
            (6, 65, 1),
            (8, 65, 8),
            (9, 65, 9),
            (10, 65, 10),
        ] {
            assert_eq!(
                replica.chunk_block(x, y, z),
                server.chunk_block(x, y, z),
                "block id diverged at ({x},{y},{z})"
            );
            assert_eq!(
                replica.water_meta_world(x, y, z),
                server.water_meta_world(x, y, z),
                "water meta diverged at ({x},{y},{z})"
            );
        }
        assert!(replica.is_water_source_world(IVec3::new(3, 65, 3)));

        // Every state map reads back through the public query surface.
        assert_eq!(
            replica.cell_kv_get(2, 65, 2, "testmod:heat"),
            Some(&[7u8, 1][..])
        );
        assert_eq!(
            replica.door_state_at(5, 65, 5),
            server.door_state_at(5, 65, 5)
        );
        assert_eq!(
            replica.door_state_at(5, 66, 5),
            server.door_state_at(5, 66, 5)
        );
        assert_eq!(
            replica.stair_state_at(6, 65, 6),
            server.stair_state_at(6, 65, 6)
        );
        assert_eq!(
            replica.torch_placement(IVec3::new(7, 65, 7)),
            TorchPlacement::East
        );
        assert_eq!(replica.log_axis_at(1, 65, 6), LogAxis::X);
        assert_eq!(
            replica.slab_state_at(6, 65, 1),
            server.slab_state_at(6, 65, 1)
        );
        assert_eq!(
            replica
                .section_at_world_for_test(2, 65, 6)
                .unwrap()
                .sapling_stage(2, 1, 6),
            2
        );
        assert_eq!(
            replica.model_offset_at(11, 65, 10),
            server.model_offset_at(11, 65, 10)
        );
        assert_eq!(replica.model_facing_at(10, 65, 10), Facing::East);
        let mut chests = Vec::new();
        replica.collect_chests(&mut chests);
        assert!(
            chests
                .iter()
                .any(|&(p, f, ..)| p == IVec3::new(1, 65, 1) && f == Facing::West),
            "the chest renders on the replica with its facing"
        );
        assert_eq!(
            replica
                .section_at_world_for_test(4, 65, 4)
                .unwrap()
                .entity_facing(4, 1, 4),
            Facing::South
        );
        assert!(
            replica
                .section_at_world_for_test(4, 65, 4)
                .unwrap()
                .is_furnace_lit(4, 1, 4),
            "the lit furnace face replicates"
        );

        // Absent sections answer physics/placement from the column summaries.
        assert!(!replica.sections.contains_key(&held_back));
        assert_eq!(replica.physics_block(2, -20, 2), Block::Stone);
        assert!(!replica.placement_cell_open(IVec3::new(2, -20, 2)));

        // Light is server-owned: installs queue MESH work only. A lightless
        // payload installs light-CLEAN (the ship gate only lets one through
        // when it never bakes) — the replica must never queue its own bake.
        assert!(replica.dirty_mesh_count() > 0, "installs queue mesh work");
        assert!(
            !replica
                .section_at_world_for_test(2, 65, 2)
                .unwrap()
                .light_dirty,
            "a replica install never queues a replica-side bake"
        );
    }

    /// Deliverable C2c-ii(1): deltas carry the cell's sparse block STATE using
    /// the save-codec encodings, and a fresh replica applying them converges
    /// on every state map — placements whose state lands AFTER the announcing
    /// block write (chest facing, torch placement) included, because the drain
    /// re-reads the maps.
    #[test]
    fn deltas_carry_cell_state_and_replicas_converge_on_it() {
        use crate::net::protocol::CellState;

        let (mut server, mut replica) = server_and_replica();
        // Converge on the pristine floor first (the delta path needs installed
        // sections on the replica).
        let columns: Vec<_> = server
            .columns
            .keys()
            .copied()
            .map(|cp| server.column_payload(cp).expect("column loaded"))
            .collect();
        let sections: Vec<_> = server.sections.values().map(|s| s.to_payload()).collect();
        for c in columns {
            replica.install_remote_column(c);
        }
        for s in sections {
            replica.install_remote_section(s);
        }

        server.set_replication_capture(true);
        let stair = IVec3::new(2, 65, 2);
        let torch = IVec3::new(3, 65, 3);
        let door = IVec3::new(4, 65, 4);
        let slab = IVec3::new(6, 65, 2);
        let log = IVec3::new(7, 65, 3);
        let model = IVec3::new(10, 65, 10);
        let chest = IVec3::new(1, 65, 1);
        assert!(server.place_stair(
            stair,
            Block::OakStairs,
            crate::block_state::StairState::new(Facing::South, crate::block_state::StairHalf::Top),
        ));
        assert!(server.set_block_world(torch.x, torch.y, torch.z, Block::Torch));
        server.insert_torch(torch, TorchPlacement::East);
        assert!(server.place_door(door, Block::OakDoor, Facing::East));
        assert!(server.place_slab_layer(
            slab,
            Block::CobblestoneSlab,
            SlabSlot {
                split: SlabSplit::Y,
                index: 0,
            },
        ));
        assert!(server.place_log(log, Block::OakLog, LogAxis::X));
        assert!(server.place_model_block_facing(model, Block::FurnitureWorkbench, Facing::East));
        assert!(server.set_block_world(chest.x, chest.y, chest.z, Block::Chest));
        server.insert_chest(chest, Facing::West);

        let deltas = server.take_block_deltas();
        let state_at = |pos: IVec3| {
            deltas
                .iter()
                .find(|d| d.pos == pos)
                .unwrap_or_else(|| panic!("delta logged at {pos:?}"))
                .state
        };
        assert!(matches!(state_at(stair), Some(CellState::Stair(_))));
        assert!(matches!(state_at(torch), Some(CellState::Torch(_))));
        assert!(matches!(state_at(door), Some(CellState::Door(_))));
        assert!(matches!(
            state_at(door + IVec3::Y),
            Some(CellState::Door(_))
        ));
        let Some(CellState::Slab([_, a, b])) = state_at(slab) else {
            panic!("slab delta carries the 3-byte record");
        };
        assert_eq!(
            (a, b),
            (Block::CobblestoneSlab.id(), Block::Air.id()),
            "slab layers ride as raw block ids"
        );
        assert!(matches!(state_at(log), Some(CellState::LogAxis(_))));
        assert!(matches!(state_at(model), Some(CellState::ModelCell { .. })));
        assert!(
            matches!(state_at(chest), Some(CellState::Facing(_))),
            "the chest facing inserted AFTER set_block_world still rides the delta"
        );

        for d in &deltas {
            replica.apply_remote_delta(*d);
        }
        assert_eq!(
            replica.stair_state_at(stair.x, stair.y, stair.z),
            server.stair_state_at(stair.x, stair.y, stair.z)
        );
        assert_eq!(replica.torch_placement(torch), TorchPlacement::East);
        assert_eq!(
            replica.door_state_at(door.x, door.y, door.z),
            server.door_state_at(door.x, door.y, door.z)
        );
        assert_eq!(
            replica.door_state_at(door.x, door.y + 1, door.z),
            server.door_state_at(door.x, door.y + 1, door.z)
        );
        assert_eq!(
            replica.slab_state_at(slab.x, slab.y, slab.z),
            server.slab_state_at(slab.x, slab.y, slab.z)
        );
        assert_eq!(replica.log_axis_at(log.x, log.y, log.z), LogAxis::X);
        assert_eq!(
            replica.model_offset_at(model.x + 1, model.y, model.z),
            server.model_offset_at(model.x + 1, model.y, model.z)
        );
        assert_eq!(
            replica.model_facing_at(model.x, model.y, model.z),
            Facing::East
        );
        let mut chests = Vec::new();
        replica.collect_chests(&mut chests);
        assert!(
            chests
                .iter()
                .any(|&(p, f, ..)| p == chest && f == Facing::West),
            "the chest placed post-join renders on the replica with its facing"
        );

        // Breaking the stair clears the replicated state too (state: None).
        server.set_replication_capture(true);
        assert!(server.set_block_world(stair.x, stair.y, stair.z, Block::Air));
        for d in server.take_block_deltas() {
            replica.apply_remote_delta(d);
        }
        assert_eq!(
            replica.stair_state_at(stair.x, stair.y, stair.z),
            crate::block_state::StairState::default(),
            "a cleared cell reads the default state again"
        );
    }

    /// Deliverable C2c-ii(2): a door TOGGLE flips the door map with no
    /// block-id write — it must still log deltas (state carries the open bit)
    /// and the replica's door map must follow, so collision + the resting
    /// swing angle are right.
    #[test]
    fn door_toggles_replicate_the_open_bit_without_a_block_change() {
        let (mut server, mut replica) = server_and_replica();
        let base = IVec3::new(5, 65, 5);
        assert!(server.place_door(base, Block::OakDoor, Facing::East));
        let columns: Vec<_> = server
            .columns
            .keys()
            .copied()
            .map(|cp| server.column_payload(cp).expect("column loaded"))
            .collect();
        let sections: Vec<_> = server.sections.values().map(|s| s.to_payload()).collect();
        for c in columns {
            replica.install_remote_column(c);
        }
        for s in sections {
            replica.install_remote_section(s);
        }
        assert!(!replica.door_state_at(base.x, base.y, base.z).unwrap().open);

        server.set_replication_capture(true);
        assert_eq!(server.toggle_door(base), Some(base));
        let deltas = server.take_block_deltas();
        assert_eq!(deltas.len(), 2, "both door cells log a delta on toggle");
        for d in deltas {
            replica.apply_remote_delta(d);
        }
        for cell in [base, base + IVec3::Y] {
            let got = replica.door_state_at(cell.x, cell.y, cell.z).unwrap();
            assert!(got.open, "the replica's door map opened at {cell:?}");
            assert_eq!(
                Some(got),
                server.door_state_at(cell.x, cell.y, cell.z),
                "replica and server door state agree"
            );
        }

        // And back closed.
        assert_eq!(server.toggle_door(base), Some(base));
        for d in server.take_block_deltas() {
            replica.apply_remote_delta(d);
        }
        assert!(!replica.door_state_at(base.x, base.y, base.z).unwrap().open);
    }

    #[test]
    fn replication_log_coalesces_latest_wins_and_respects_capture() {
        let mut w = crate::world::testutil::flat_world();
        assert!(w.set_block_world(2, 70, 2, Block::Stone));
        assert!(w.take_block_deltas().is_empty(), "capture off logs nothing");

        w.set_replication_capture(true);
        assert!(w.set_block_world(3, 70, 3, Block::Stone));
        assert!(w.set_block_world(3, 70, 3, Block::Dirt)); // same cell, same tick
        assert!(w.set_water_world(IVec3::new(4, 70, 4), Block::Water, 0x83));
        let deltas = w.take_block_deltas();
        assert_eq!(deltas.len(), 2, "one delta per cell per take");
        let cell = deltas
            .iter()
            .find(|d| d.pos == IVec3::new(3, 70, 3))
            .expect("edited cell logged");
        assert_eq!(cell.block_id, Block::Dirt.id(), "latest write wins");
        assert_eq!(cell.water, None);
        let water = deltas
            .iter()
            .find(|d| d.pos == IVec3::new(4, 70, 4))
            .expect("water cell logged");
        assert_eq!(water.block_id, Block::Water.id());
        assert_eq!(water.water, Some(0x83), "water meta rides the delta");
        assert!(w.take_block_deltas().is_empty(), "take drains the log");
    }

    /// The per-connection send plan: wanted loaded+FINAL sections ship (both
    /// finality gates: in-flight streaming AND light not yet baked), sent
    /// terrain leaving the keep shape (or the server) plans an unload, and the
    /// send key is stable across pumps that change nothing — the
    /// incrementality gate.
    #[test]
    fn terrain_send_plan_gates_finality_and_unloads_the_keep_shape_exit() {
        use crate::chunk::SECTION_VOLUME;
        use crate::section::Section;
        use crate::world::store::LoadAnchor;
        use rustc_hash::FxHashSet;

        let sky = || Arc::from(vec![0u8; SECTION_VOLUME].into_boxed_slice());
        let mut w = World::new(0, 2);
        let sp = SectionPos::new(0, 4, 0);
        let mut section = Section::new(0, 4, 0);
        section.set_block(0, 0, 0, Block::Stone);
        w.insert_section_for_test(sp, section);
        let anchor = |cx: i32| LoadAnchor {
            cx,
            cy: 4,
            cz: 0,
            radius: 64,
        };

        let mut sent_columns: FxHashSet<ChunkPos> = FxHashSet::default();
        let mut sent_sections: FxHashSet<SectionPos> = FxHashSet::default();
        // Light gates shipping: a never-baked (non-opaque) section is not
        // presentable — the replica can't bake it, so the server holds it.
        let plan = w.plan_terrain_send(anchor(0), &sent_columns, &sent_sections, 128);
        assert!(
            !plan.sections.contains(&sp),
            "a lightless section is held back by the ship gate"
        );
        w.section_at_world_mut_for_test(0, 64, 0)
            .unwrap()
            .set_skylight(sky());
        let plan = w.plan_terrain_send(anchor(0), &sent_columns, &sent_sections, 128);
        assert!(
            plan.sections.contains(&sp),
            "the loaded, lit, wanted section ships"
        );
        sent_columns.insert(sp.chunk_pos());
        sent_sections.insert(sp);

        // The send key: stable while nothing moved; re-keyed by new content
        // and by an anchor chunk move.
        let k = w.terrain_send_key(anchor(0));
        assert_eq!(k, w.terrain_send_key(anchor(0)));
        assert_ne!(k, w.terrain_send_key(anchor(1)), "a chunk move re-keys");
        let mut other = Section::new(1, 4, 0);
        other.set_block(0, 0, 0, Block::Stone);
        other.set_skylight(sky());
        w.insert_section_for_test(SectionPos::new(1, 4, 0), other);
        assert_ne!(k, w.terrain_send_key(anchor(0)), "new content re-keys");

        // A loaded section whose saved overlay is still in flight is NOT
        // final: it must not ship until the overlay resolves.
        w.awaited_overlays.insert(SectionPos::new(1, 4, 0));
        let plan = w.plan_terrain_send(anchor(0), &sent_columns, &sent_sections, 128);
        assert!(
            !plan.sections.contains(&SectionPos::new(1, 4, 0)),
            "an in-flight section must not be sent (its base would lie)"
        );
        w.awaited_overlays.clear();
        let plan = w.plan_terrain_send(anchor(0), &sent_columns, &sent_sections, 128);
        assert!(plan.sections.contains(&SectionPos::new(1, 4, 0)));

        // A sent section the server evicted (vertical exit) unloads even while
        // its column is kept.
        let gone = SectionPos::new(0, 9, 0);
        sent_sections.insert(gone);
        let plan = w.plan_terrain_send(anchor(0), &sent_columns, &sent_sections, 128);
        assert!(plan.drop_sections.contains(&gone));
        assert!(plan.drop_columns.is_empty());
        sent_sections.remove(&gone);

        // The whole column leaving the keep shape plans a ColumnUnload (its
        // sections drop with it — no per-section messages).
        let plan = w.plan_terrain_send(anchor(20), &sent_columns, &sent_sections, 128);
        assert!(plan.drop_columns.contains(&sp.chunk_pos()));
        assert!(!plan.drop_sections.contains(&sp));
    }

    #[test]
    fn sealed_mixed_section_is_not_final_without_light() {
        let mut world = World::new(0, 16);
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
        world.last_load_target = Some(LoadTarget::new(8, 0, 0, 16));

        assert!(world.section_sealed_by_loaded_neighbors(center));
        assert!(
            !world.section_light_final(center),
            "a mixed section needs real light before replication can call it final"
        );
    }

    #[test]
    fn server_headless_never_queues_mesh_work() {
        let mut headless = World::new_with_role(0, 1, WorldRole::ServerHeadless);
        headless.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(headless.set_block_world(8, 64, 8, Block::Stone));
        assert_eq!(
            headless.dirty_mesh_count(),
            0,
            "nobody pumps a headless world's meshes; the queue must stay empty"
        );
        // Light bookkeeping still runs — the server keeps light current.
        assert!(
            headless
                .section_at_world_for_test(8, 64, 8)
                .unwrap()
                .light_dirty
        );

        let mut combined = World::new(0, 1);
        combined.insert_empty_column_for_test(ChunkPos::new(0, 0));
        assert!(combined.set_block_world(8, 64, 8, Block::Stone));
        assert!(
            combined.dirty_mesh_count() > 0,
            "the combined world still queues meshes as before"
        );
    }

    /// Per-connection view distance: the send shape follows the anchor's own
    /// radius, clamped by the server world's budget — a client may shrink its
    /// stream but never widen it past the server setting.
    #[test]
    fn send_target_clamps_anchor_radius_to_the_world_budget() {
        use crate::world::LoadAnchor;
        let w = World::new_with_role(0, 4, WorldRole::ServerHeadless);
        let key = |radius| {
            w.terrain_target_key(LoadAnchor {
                cx: 0,
                cy: 4,
                cz: 0,
                radius,
            })
        };
        assert_eq!(key(64), key(4), "requests above the budget clamp to it");
        assert_ne!(key(2), key(4), "smaller requests shrink the send shape");
    }
}
