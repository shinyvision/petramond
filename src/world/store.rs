use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use petramond_world::block::Block;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_mesh::ChunkMesh;
use crate::mob::Mobs;
use crate::save::WorldSave;
use petramond_world::section::{Section, SectionSummary};
use crate::worker::{JobCancel, JobPool, WorkerPool};
use petramond_worldgen::driver::ChunkGenerator;
use petramond_worldgen::driver::ColumnGen;

use super::entities::DroppedItems;
use petramond_world::world::saved_index::SavedIndex;
use petramond_world::world::environment::WorldEnvironment;
use super::light::LightBakeQueue;
use super::mesh_queue::DirtyMeshQueue;
use petramond_world::world::tick_state::TickState;


// Moved halves, re-exported under their historical `store::` paths.
pub use petramond_world::world::column_heightmaps::SkyCoverChange;
pub use petramond_world::world::data::{ModSimState, WorldData, WorldRole};
pub use petramond_world::world::load_targets::{
    LoadAnchor, LoadTarget, RENDER_DIST, VERTICAL_LOAD_RADIUS,
};

mod block_entity_index;
mod evict;
mod memory;
mod mesh_index;
mod section_index;

pub use memory::MemoryCensus;

#[cfg(any(test, feature = "test-support"))]
mod fixtures;
#[cfg(test)]
mod tests;

/// The cubic voxel world: a sparse 3D grid of 16³ [`Section`]s plus a sparse 2D
/// grid of per-column [`Column`] data (biome, visible surface, direct-sky cover).
/// Sections are the unit of storage, meshing, lighting, streaming, and saving; a
/// column exists whenever any of its sections is loaded (see
/// [`ensure_column`](World::ensure_column)).
/// The replica's terrain presentation: the CPU section meshes, the packed
/// column bookkeeping the GPU upload reads, the bake job pool, and the
/// visibility parking sets.
///
/// Owned by `WorldRole::ClientReplica` (and `Combined` for dev tooling). A
/// `ServerHeadless` world allocates it empty and never writes it — see
/// `World::queue_dirty_mesh`, which early-returns for that role.
pub(in crate::world) struct TerrainRenderState {
    /// One GPU-ready mesh per section.
    pub(in crate::world) meshes: FxHashMap<SectionPos, ChunkMesh>,
    /// XZ columns that currently have at least one CPU section mesh.
    /// Mirrors `meshes` so renderer retention does not scan the vertical range
    /// of every GPU column each frame.
    pub(in crate::world) mesh_columns: FxHashSet<ChunkPos>,
    /// Per-column bitset of meshed section `cy` values (bit `i` =
    /// `SECTION_MIN_CY + i`). Kept in sync with `meshes` / `mesh_columns` so
    /// packed-column consumers walk only the meshed stack, not the full
    /// vertical world range.
    pub(in crate::world) mesh_column_cys: FxHashMap<ChunkPos, u32>,
    /// Changes whenever a section mesh enters or leaves a packed GPU column.
    /// The renderer uses it to coalesce consecutive sibling completions.
    pub(in crate::world) mesh_upload_revisions: FxHashMap<ChunkPos, u64>,
    /// XZ columns whose packed render buffer must be rebuilt from `meshes`.
    /// Kept explicitly so the renderer does not scan every section mesh each frame.
    pub(in crate::world) mesh_upload_dirty_columns: FxHashSet<ChunkPos>,
    /// Columns a synchronous click presentation just installed meshes into.
    /// The renderer drains these each frame and uploads them without waiting
    /// out its quiet-gate coalescing — the player is pointing at them, so
    /// coalescing latency is exactly the wrong trade there.
    pub(in crate::world) upload_urgent_columns: FxHashSet<ChunkPos>,
    /// Uploaded columns scheduled to release their CPU mesh buffers once they have
    /// been upload-quiet long enough (value = earliest release frame). The retained
    /// CPU copy exists only so a column repack can re-pack sibling sections; a
    /// settled column frees it and repacks force a remesh instead (`repack_forced`).
    pub(in crate::world) mesh_release_after: FxHashMap<ChunkPos, u64>,
    /// Released sections whose column needs a GPU repack: their remesh must not be
    /// skipped by deep-visibility parking — the packed column buffer cannot be
    /// rebuilt without their geometry.
    pub(in crate::world) repack_forced: FxHashSet<SectionPos>,
    /// Monotonic mesh-pump frame counter (drives `mesh_release_after`).
    pub(in crate::world) mesh_pump_frame: u64,
    /// Ordinary off-thread section meshing: dirty sections are submitted as owned
    /// snapshots and finished meshes drained back. Local prediction deliberately
    /// invokes the same builder synchronously.
    pub(in crate::world) mesh_pool: super::mesh_pool::MeshPool,
    pub(in crate::world) mesh_jobs_in_flight: usize,
    /// Latest mesh job per section. Re-dirtying cancels queued stale work;
    /// completion tokens prevent an older result from clearing a newer handle.
    pub(in crate::world) mesh_job_cancels: FxHashMap<SectionPos, JobCancel>,
    pub(in crate::world) dirty_meshes: DirtyMeshQueue,
    /// Loaded sections wholly below their column's surface retention band — only
    /// visible through cave openings (see `world::visibility`).
    pub(in crate::world) deep_sections: FxHashSet<SectionPos>,
    /// The deep sections the last visibility refresh could reach from the visible
    /// region. Deep sections outside this set park instead of meshing.
    pub(in crate::world) visible_deep: FxHashSet<SectionPos>,
    /// Dirty deep sections parked because nothing can see them. Re-queued by the
    /// visibility refresh when they become reachable (or the player ring arrives).
    pub(in crate::world) hidden_parked: FxHashSet<SectionPos>,
    /// Dirty sections whose six exact loaded neighbour planes currently seal them
    /// from outside sightlines. Kept separate from deep visibility so a load-target
    /// move can wake them when a player may already be inside.
    pub(in crate::world) sealed_parked: FxHashSet<SectionPos>,
    /// Asynchronous reconciliation light -> mesh bundles. Initial prediction
    /// runs the same complete invalidation footprint synchronously.
    pub(in crate::world) prediction_terrain: super::prediction_render::PredictionTerrainQueue,
    /// Raised by ingest / edits / load-target moves; consumed by the mesh pump,
    /// which re-runs the deep-visibility BFS before submitting work.
    pub(in crate::world) vis_dirty: bool,
    /// Dirty meshes parked while async light bakes their sampling neighbourhood.
    /// They re-enter `dirty_meshes` only once the 3×3×3 light dependency set is clean.
    pub(in crate::world) light_blocked_meshes: FxHashSet<SectionPos>,
}

/// The SERVER's worldgen + disk streaming work: which columns/sections are
/// generating or awaited, the overlay handshake, and the column records
/// queued for persistence.
///
/// Owned by `WorldRole::ServerHeadless` (and `Combined`). A `ClientReplica`
/// never generates — its sections arrive as remote payloads — so it carries
/// this empty.
pub(in crate::world) struct WorldgenJobs {
    /// Columns whose shared 2D gen data (`ColumnGen`) has landed: the source for
    /// submitting per-section jobs and sizing each column's vertical load window.
    /// Present for every loaded column; dropped when the column unloads.
    pub(in crate::world) column_gen: FxHashMap<ChunkPos, Arc<ColumnGen>>,
    /// Columns queued for the (heavy, once-per-column) `ColumnGen` job.
    pub(in crate::world) pending: FxHashMap<ChunkPos, Option<JobCancel>>,
    /// Sections with an in-flight per-section gen job, so the streamer never submits a
    /// section twice while it is being generated.
    pub(in crate::world) pending_sections: FxHashSet<SectionPos>,
    /// Count of `pending_sections` per XZ column. Lets settled-column slimming
    /// ask "anything still pending in this column?" in O(1) instead of rebuilding
    /// a column set from every pending section each ingest pump.
    pub(in crate::world) pending_section_columns: FxHashMap<ChunkPos, u16>,
    /// Cancellation handles for pending worker-generated sections. Disk-primary
    /// requests are in `pending_sections` without an entry here.
    pub(in crate::world) pending_section_jobs: FxHashMap<SectionPos, JobCancel>,
    /// Saved (player-modified) sections read back from disk whose generated column has
    /// not arrived yet — disk I/O usually beats noise-gen. Held here until the column
    /// lands, then overlaid over the generated terrain (see `world::stream::poll`).
    pub(in crate::world) pending_overlays: FxHashMap<SectionPos, super::stream::LoadedOverlay>,
    /// Sections whose saved record has been REQUESTED from the save thread but not
    /// answered yet. Until the answer lands (and any overlay applies) the section's
    /// true content is in flight: the sim guard blocks mutation and the harvest skips
    /// persisting it (see `world::sim_guard`).
    pub(in crate::world) awaited_overlays: FxHashSet<SectionPos>,
    /// Requested disk records that install as the section's PRIMARY content — no
    /// gen job was submitted for them ("Optimize explored terrain"). A corrupt
    /// answer falls back to generation; see `world::stream::submit_section_job`.
    pub(in crate::world) disk_primary_sections: FxHashSet<SectionPos>,
    /// Column-gen cache records awaiting a batched write: buffered so the
    /// save thread merges many columns per region file rewrite instead of
    /// read-modify-writing per column. Records are pure gen data — a crash
    /// losing the buffer only costs a future regen.
    pub(in crate::world) pending_colgen_records: Vec<crate::save::colgen::ColumnGenRecord>,
    /// Chunk columns whose one-time worldgen herd actually spawned (see
    /// `mob::populate`) — the fact that keeps the initial animal stock from
    /// re-minting every session. Persisted in `level.dat`; BTreeSet so the
    /// encoding iterates in one deterministic order. Mutated on the tick only.
    pub(in crate::world) populated_columns: BTreeSet<ChunkPos>,
}

/// The SERVER's per-tick change log: what to ship to each session next
/// batch. Captured only while `replication_capture` is on.
pub(in crate::world) struct ReplicationLog {
    /// Server-side replication log gate; ~zero cost while off (one branch at
    /// the block-change choke point). See [`set_replication_capture`].
    ///
    /// [`set_replication_capture`]: Self::set_replication_capture
    pub(in crate::world) replication_capture: bool,
    /// This tick's coalesced block/water changes, latest state per cell —
    /// drained by [`take_block_deltas`](Self::take_block_deltas).
    pub(in crate::world) block_delta_log:
        FxHashMap<petramond_math::math::IVec3, crate::net::protocol::BlockDelta>,
    /// This tick's coalesced per-cell mod KV changes, latest value per
    /// `(pos, key)` — drained by
    /// [`take_cell_kv_deltas`](Self::take_cell_kv_deltas). Captured behind
    /// the same [`replication_capture`](Self::replication_capture) gate.
    pub(in crate::world) cell_kv_delta_log:
        FxHashMap<(petramond_math::math::IVec3, String), Option<Vec<u8>>>,
    /// Cells whose mod DRAW SET changed this tick — drained by
    /// [`take_block_draw_deltas`](Self::take_block_draw_deltas) as whole sets
    /// (they are a handful of prims, and half a set draws nothing sensible).
    pub(in crate::world) block_draw_log: FxHashSet<petramond_math::math::IVec3>,
    /// ServerHeadless only: sections whose bake LANDED since the last
    /// streaming pump — drained by [`take_light_ship_log`](Self::take_light_ship_log)
    /// into per-connection `LightData` messages (filtered to each recipient's
    /// sent set). A set, so several bakes in one window ship latest-wins.
    pub(in crate::world) light_ship_log: FxHashSet<SectionPos>,
    /// Monotonic revision of "which sections exist / are stream-final": bumped
    /// on ingest, eviction, materialization, and in-flight-set changes. The
    /// per-connection terrain sender keys its wanted-vs-sent rescan on this
    /// (plus the anchor's quantized target), so a steady frame does no scan.
    pub(in crate::world) terrain_revision: u64,
}

/// Per-world session state the SERVER owns: who is connected, their queued
/// inputs, and the world rules their actions read.
pub(in crate::world) struct SessionState {
    /// Every connected player's movement intent this tick, decomposed into
    /// its own yaw frame (see [`crate::player::PlayerInputSnapshot`]) —
    /// published by the server before the tick stages so the `PlayerInput`
    /// HostCall can answer from the world. Replaced wholesale each tick.
    pub(in crate::world) player_inputs: Vec<crate::player::PlayerInputSnapshot>,
    /// Every connected player's state snapshot this tick (see
    /// [`crate::player::PlayerRosterSnapshot`]) — published beside the
    /// inputs; the read model behind the `Players` HostCall. Replaced
    /// wholesale each tick.
    pub(in crate::world) player_roster: Vec<crate::player::PlayerRosterSnapshot>,
    /// Keep the inventory on death instead of spilling it (per-world
    /// `settings.json` rule). Session-fixed, set once at open.
    pub(in crate::world) keep_inventory: bool,
    /// The full day+night cycle length in ticks (per-world `settings.json`
    /// "day length"). Session-fixed, set once at open BEFORE core systems
    /// install — the day/night cycle captures it.
    pub(in crate::world) day_cycle_ticks: u64,
}

/// Mod-owned SIM state: the block hooks packs registered, their key/value
/// store, the disabled set, and the custom-shape bake cache their WASM fills.
/// Deterministic tick state — lives on [`WorldData`].
pub(in crate::world) struct ModStreamState {
    /// Per-cell mod DRAW SETS: retained presentation geometry a mod submits
    /// for a placed block, redrawn every frame with no re-mesh. Sparse (empty
    /// in almost every world) and per-cell, exactly like `custom_bake` — mod
    /// state, so it lives with the rest of it rather than on the root.
    pub(in crate::world) block_draws:
        FxHashMap<petramond_math::math::IVec3, crate::world::draw::PlacedDraw>,
    /// The same sets indexed by the SECTION their anchor sits in, with a union
    /// bound per section. Every consumer of this store is section-shaped (a
    /// section payload, an eviction, a frame's view cull), and each of them
    /// walked the whole map before this index existed.
    pub(in crate::world) block_draw_sections:
        FxHashMap<petramond_world::chunk::SectionPos, crate::world::draw::SectionDraws>,
    /// Section installs the per-frame streamer buffered for the tick-side event bus
    /// (`section_generated` / `section_loaded`); drained by the next game tick.
    pub(in crate::world) stream_events: Vec<super::stream::StreamEvent>,
    /// Buffer gate, mirroring event-bus listener presence (set once per tick), so
    /// streaming costs nothing while nothing listens.
    pub(in crate::world) stream_events_enabled: bool,
}

/// The DETERMINISTIC half of the world: sparse section/column storage, the
/// fixed-timestep tick state, and every pure query over them.
///
pub struct World {
    /// The deterministic world half. `World` derefs here, so `world.foo()`
    /// reaches both halves; orchestration code that split-borrows writes
    /// `self.data.` explicitly.
    pub data: WorldData,
    /// CLIENT-side terrain presentation. Dead weight on a headless server:
    /// `queue_dirty_mesh` early-returns for `ServerHeadless`, so nothing here
    /// is ever filled there. Grouped so that is VISIBLE rather than spread
    /// across twenty fields the server carries for nothing.
    pub(in crate::world) terrain: TerrainRenderState,
    /// SERVER-side worldgen/streaming job tables; empty on a replica.
    pub(in crate::world) gen: WorldgenJobs,
    /// SERVER-side replication capture; inert on a replica.
    pub(in crate::world) replication: ReplicationLog,
    /// SERVER-side session roster + world rules.
    pub(in crate::world) session: SessionState,
    /// Mod-owned streaming/draw state (retained draws, stream events).
    pub(in crate::world) mod_stream: ModStreamState,
    /// This tick's shared navigation reachability-probe budget (see
    /// `mob::nav::REACH_PROBE_TICK_BUDGET`), refilled by `tick_mobs`. It lives
    /// here because both askers — the mob brains and the mod ABI's
    /// `MobCanReach` — hold only `&World` when they ask.
    nav_probe_budget: crate::mob::ReachBudget,
    pub worker: WorkerPool,
    pub(super) light_bakes: LightBakeQueue,
    /// On-disk save handle (`None` if saving is disabled / failed to open).
    pub(super) save: Option<WorldSave>,
    /// Active dropped item entities resting in currently-loaded sections.
    pub(super) dropped_items: DroppedItems,
    /// Active mobs in currently-loaded sections.
    pub(super) mobs: Mobs,
    /// Player-on-mob riding attachments (see `mob::riding`). On `World` so the
    /// mount HostCalls reach it through `SimCtx`; the server's riding pass
    /// reconciles sessions against it each tick. Never persisted.
    pub(super) riding: crate::mob::riding::Riding,
}

impl std::ops::Deref for World {
    type Target = WorldData;
    #[inline]
    fn deref(&self) -> &WorldData {
        &self.data
    }
}

impl std::ops::DerefMut for World {
    #[inline]
    fn deref_mut(&mut self) -> &mut WorldData {
        &mut self.data
    }
}

impl World {
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self::new_with_role(seed, render_dist, WorldRole::Combined)
    }

    pub fn new_with_role(seed: u32, render_dist: i32, role: WorldRole) -> Self {
        // ONE background pool shared by every streaming stage; the per-stage adapters
        // below each hold a handle and compete purely on distance priority.
        let jobs = std::sync::Arc::new(JobPool::new(JobPool::default_threads()));
        Self::new_with_pool(seed, render_dist, role, jobs)
    }

    /// Construct over a caller-owned job pool, so the server world and the
    /// local client's replica — which run in one process — can share one pool
    /// instead of each spawning a machine-sized thread set.
    pub fn new_with_pool(
        seed: u32,
        render_dist: i32,
        role: WorldRole,
        jobs: std::sync::Arc<JobPool>,
    ) -> Self {
        Self {
            data: WorldData {
                seed,
                role,
                sections: FxHashMap::default(),
                columns: FxHashMap::default(),
                column_payload_revisions: FxHashMap::default(),
                column_revision_counter: 0,
                section_column_cys: FxHashMap::default(),
                section_column_rt: FxHashMap::default(),
                random_tick_dirty: FxHashSet::default(),
                render_dist,
                lighting_revision: 0,
                block_entity_sections: FxHashSet::default(),
                particle_emitter_sections: FxHashSet::default(),
                light_deferred: FxHashSet::default(),
                deferred_recheck_needed: false,
                deferred_rechecks: FxHashSet::default(),
                last_load_target: None,
                extra_load_targets: Vec::new(),
                missing_columns_settled: false,
                column_summaries: FxHashMap::default(),
                column_biome_halos: FxHashMap::default(),
                column_deep_band_los: FxHashMap::default(),
                relight_demand: FxHashSet::default(),
                relit_since_persist: FxHashSet::default(),
                light_edited_since_persist: FxHashSet::default(),
                sim: TickState::new(seed),
                environment: WorldEnvironment::default(),
                mods: ModSimState {
                    mod_block_hooks: Vec::new(),
                    mod_kv: BTreeMap::new(),
                    disabled_mods: std::collections::BTreeSet::new(),
                    custom_bake: FxHashMap::default(),
                    custom_bake_dirty: FxHashSet::default(),
                },
                stream_nonfinal: FxHashSet::default(),
                saved: SavedIndex::default(),
            },
            terrain: TerrainRenderState {
                meshes: FxHashMap::default(),
                mesh_columns: FxHashSet::default(),
                mesh_column_cys: FxHashMap::default(),
                mesh_upload_revisions: FxHashMap::default(),
                mesh_upload_dirty_columns: FxHashSet::default(),
                upload_urgent_columns: FxHashSet::default(),
                mesh_release_after: FxHashMap::default(),
                repack_forced: FxHashSet::default(),
                mesh_pump_frame: 0,
                prediction_terrain: super::prediction_render::PredictionTerrainQueue::new(
                    jobs.clone(),
                ),
                mesh_pool: super::mesh_pool::MeshPool::new(jobs.clone()),
                mesh_jobs_in_flight: 0,
                mesh_job_cancels: FxHashMap::default(),
                dirty_meshes: DirtyMeshQueue::default(),
                deep_sections: FxHashSet::default(),
                visible_deep: FxHashSet::default(),
                hidden_parked: FxHashSet::default(),
                sealed_parked: FxHashSet::default(),
                vis_dirty: false,
                light_blocked_meshes: FxHashSet::default(),
            },
            gen: WorldgenJobs {
                column_gen: FxHashMap::default(),
                pending: FxHashMap::default(),
                pending_sections: FxHashSet::default(),
                pending_section_columns: FxHashMap::default(),
                pending_section_jobs: FxHashMap::default(),
                pending_overlays: FxHashMap::default(),
                awaited_overlays: FxHashSet::default(),
                disk_primary_sections: FxHashSet::default(),
                pending_colgen_records: Vec::new(),
                populated_columns: BTreeSet::new(),
            },
            replication: ReplicationLog {
                replication_capture: false,
                block_delta_log: FxHashMap::default(),
                cell_kv_delta_log: FxHashMap::default(),
                block_draw_log: FxHashSet::default(),
                terrain_revision: 0,
                light_ship_log: FxHashSet::default(),
            },
            session: SessionState {
                player_inputs: Vec::new(),
                player_roster: Vec::new(),
                keep_inventory: false,
                day_cycle_ticks: crate::server::daynight::DEFAULT_CYCLE_TICKS,
            },
            mod_stream: ModStreamState {
                block_draws: FxHashMap::default(),
                block_draw_sections: FxHashMap::default(),
                stream_events: Vec::new(),
                stream_events_enabled: false,
            },
            nav_probe_budget: crate::mob::ReachBudget::default(),
            worker: WorkerPool::new(jobs.clone()),
            light_bakes: LightBakeQueue::new(jobs.clone()),
            save: None,
            dropped_items: DroppedItems::default(),
            mobs: Mobs::new(seed as u64),
            riding: Default::default(),
        }
    }

    /// Record `sp` as stream-nonfinal (an in-flight gen job, awaited saved
    /// record, or pending overlay was just registered). Call beside EVERY
    /// insert into one of `WorldgenJobs`' three in-flight sets.
    #[inline]
    pub(super) fn note_stream_nonfinal(&mut self, sp: SectionPos) {
        self.data.stream_nonfinal.insert(sp);
    }

    /// Re-derive `sp`'s stream-nonfinal membership after a removal from one of
    /// the three in-flight sets: it stays nonfinal while ANY of them still
    /// holds it. Self-healing — call beside every remove.
    #[inline]
    pub(super) fn settle_stream_nonfinal(&mut self, sp: SectionPos) {
        if !self.gen.pending_sections.contains(&sp)
            && !self.gen.awaited_overlays.contains(&sp)
            && !self.gen.pending_overlays.contains_key(&sp)
        {
            self.data.stream_nonfinal.remove(&sp);
        }
    }

    /// Install a column's landed gen data AND derive its absent-section
    /// summaries — the one entry point (streaming, tests) so the deterministic
    /// half's occupancy answers can never go missing for a gen-backed column.
    pub(in crate::world) fn set_column_gen(
        &mut self,
        pos: ChunkPos,
        col: Arc<petramond_worldgen::driver::ColumnGen>,
    ) {
        let summaries: Box<[SectionSummary]> = WorldData::column_section_range()
            .map(|cy| col.section_summary(cy))
            .collect();
        self.data.column_summaries.insert(pos, summaries);
        self.gen.column_gen.insert(pos, col);
    }

    /// Rebuild `stream_nonfinal` wholesale from the three in-flight sets —
    /// the bulk (`clear`) counterpart of the per-section maintainers.
    pub(super) fn rebuild_stream_nonfinal(&mut self) {
        self.data.stream_nonfinal.clear();
        let union = self
            .gen
            .pending_sections
            .iter()
            .chain(self.gen.awaited_overlays.iter())
            .chain(self.gen.pending_overlays.keys())
            .copied()
            .collect();
        self.data.stream_nonfinal = union;
    }

    /// Keep the inventory on death (per-world `settings.json` rule).
    #[inline]
    pub fn keep_inventory(&self) -> bool {
        self.session.keep_inventory
    }

    /// Install the keep-inventory rule — once, at session open.
    pub fn set_keep_inventory(&mut self, keep: bool) {
        self.session.keep_inventory = keep;
    }

    /// The full day+night cycle length in ticks (per-world "day length").
    #[inline]
    pub fn day_cycle_ticks(&self) -> u64 {
        self.session.day_cycle_ticks
    }

    /// Install the world's cycle length — once, at session open, BEFORE core
    /// systems install (the day/night cycle captures it).
    pub fn set_day_cycle_ticks(&mut self, ticks: u64) {
        self.session.day_cycle_ticks = ticks.max(1);
    }

    /// Change the view/streaming radius live (the Options view-distance
    /// slider). On a streaming world the next `update_load*` re-shapes the
    /// working set (anchor radii clamp to this budget); on a replica it
    /// re-shapes mesh/light scheduling around the view center.
    pub fn set_render_dist(&mut self, chunks: i32) {
        let chunks = chunks.max(1);
        if self.render_dist == chunks {
            return;
        }
        self.render_dist = chunks;
        self.terrain.vis_dirty = true;
    }

    /// Replace the published per-player input snapshots for this tick (see
    /// [`crate::player::PlayerInputSnapshot`]).
    pub fn set_player_inputs(&mut self, inputs: Vec<crate::player::PlayerInputSnapshot>) {
        self.session.player_inputs = inputs;
    }

    /// The published input snapshot for `player`, if connected this tick.
    pub fn player_input(&self, player: u8) -> Option<crate::player::PlayerInputSnapshot> {
        self.session
            .player_inputs
            .iter()
            .find(|i| i.id == player)
            .copied()
    }

    /// Replace the published per-player roster for this tick (see
    /// [`crate::player::PlayerRosterSnapshot`]).
    pub fn set_player_roster(&mut self, roster: Vec<crate::player::PlayerRosterSnapshot>) {
        self.session.player_roster = roster;
    }

    /// Every connected player's published snapshot this tick, session-id order.
    pub fn player_roster(&self) -> &[crate::player::PlayerRosterSnapshot] {
        &self.session.player_roster
    }

    /// Ensure an empty section exists at `pos` so a write can land in it, materializing
    /// it (and its column) on demand. This is how building into the open air above the
    /// surface works: the streamer skips all-air sections (none are loaded there), so the
    /// first block placed in such a section springs it into being. No-op if the section is
    /// already loaded; returns `false` if `pos` is outside the world vertical range.
    pub(super) fn materialize_section(&mut self, pos: SectionPos) -> bool {
        if !SectionPos::cy_in_range(pos.cy) {
            return false;
        }
        // A section with an in-flight gen job or saved overlay is not writable: a
        // base materialized now would race the landing result, and a mutation of it
        // could be persisted and permanently shadow the real content (sim guard).
        if !self.stream_writable(pos) {
            return false;
        }
        if !self.sections.contains_key(&pos) {
            if self.saved_section_contains(pos) {
                return false;
            }
            let section = self
                .gen
                .column_gen
                .get(&pos.chunk_pos())
                .filter(|col| col.section_summary(pos.cy) != SectionSummary::Empty)
                .map(|col| ChunkGenerator::new(self.seed).generate_section(pos, col))
                .unwrap_or_else(|| Section::new(pos.cx, pos.cy, pos.cz));
            self.ensure_column(pos.chunk_pos());
            self.sections.insert(pos, Arc::new(section));
            self.note_section_loaded(pos);
            self.refresh_block_entity_index(pos);
            self.refresh_particle_emitter_index(pos);
            // A synchronously-born section must enter connected clients' sent
            // shapes promptly, or its deltas are filtered until an anchor move.
            self.bump_terrain_revision();
        }
        true
    }

    /// See [`terrain_revision`](Self::terrain_revision) (field docs).
    #[inline]
    pub fn terrain_revision(&self) -> u64 {
        self.replication.terrain_revision
    }

    /// See [`terrain_revision`](Self::terrain_revision) (field docs).
    #[inline]
    pub(super) fn bump_terrain_revision(&mut self) {
        self.replication.terrain_revision = self.replication.terrain_revision.wrapping_add(1);
    }

    /// [`materialize_section`](Self::materialize_section) for the section owning world
    /// cell `c`. Returns `false` if `c` is outside the world vertical range.
    pub(super) fn materialize_section_at(&mut self, c: petramond_math::math::IVec3) -> bool {
        match SectionPos::from_world(c.x, c.y, c.z) {
            Some(sp) => self.materialize_section(sp),
            None => false,
        }
    }

    // --- Column data ------------------------------------------------------------


    /// This tick's shared navigation probe budget.
    #[inline]
    pub fn reach_budget(&self) -> &crate::mob::ReachBudget {
        &self.nav_probe_budget
    }

}

use petramond_world::block::{Aabb, ShapeRenderBox, ShapeState};
use petramond_math::math::IVec3;
use petramond_world::block::ShapeNeighborhood;
/// `&World` coerces to `&WorldData` only at concrete argument positions, not
/// through trait bounds — so the orchestration wrapper forwards the seam.
impl ShapeNeighborhood for World {
    fn block(&self, pos: IVec3) -> Block {
        self.data.block(pos)
    }

    fn shape_state(&self, pos: IVec3) -> ShapeState {
        self.data.shape_state(pos)
    }

    fn baked(&self, pos: IVec3) -> Option<&[ShapeRenderBox]> {
        self.data.baked(pos)
    }

    fn baked_collision(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.data.baked_collision(pos)
    }
}

impl petramond_world::block::behavior::BehaviorWorld for World {
    fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: petramond_world::block::Block) -> bool {
        World::set_block_world(self, wx, wy, wz, b)
    }

    fn break_block_naturally(&mut self, pos: petramond_math::math::IVec3) {
        World::break_block_naturally(self, pos)
    }
}
