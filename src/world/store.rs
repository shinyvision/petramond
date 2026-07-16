use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{self, ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};
use crate::column::Column;
use crate::mesh::ChunkMesh;
use crate::mob::Mobs;
use crate::save::WorldSave;
use crate::section::{Section, SectionSummary};
use crate::worker::{JobCancel, JobPool, WorkerPool};
use crate::worldgen::driver::ChunkGenerator;
use crate::worldgen::driver::ColumnGen;

use super::entities::DroppedItems;
use super::environment::WorldEnvironment;
use super::light::LightBakeQueue;
use super::mesh_queue::DirtyMeshQueue;
use super::tick::TickState;

pub(super) use super::column_heightmaps::SkyCoverChange;
pub(super) use super::load_targets::LoadTarget;
pub use super::load_targets::{LoadAnchor, RENDER_DIST, VERTICAL_LOAD_RADIUS};

/// Which half of the client/server split this `World` instance plays
/// Until Phase C flips the split on, the one live world
/// is [`Combined`](WorldRole::Combined): it runs the sim AND meshes for the
/// renderer, exactly as before.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum WorldRole {
    /// Today's single world: gen + sim + light + mesh.
    #[default]
    Combined,
    /// The internal server's sim world: gen + light + sim, NO meshing — every
    /// mesh-queueing entry point is a no-op so the dirty-mesh queue cannot
    /// grow with nobody pumping it.
    ServerHeadless,
    /// A client's replica: no gen, no sim ticks. Sections are installed from
    /// the connection (`world::remote`); it computes its own light, meshes,
    /// and serves collision/raycast/placement queries.
    ClientReplica,
}

/// The cubic voxel world: a sparse 3D grid of 16³ [`Section`]s plus a sparse 2D
/// grid of per-column [`Column`] data (biome, visible surface, direct-sky cover).
/// Sections are the unit of storage, meshing, lighting, streaming, and saving; a
/// column exists whenever any of its sections is loaded (see
/// [`ensure_column`](World::ensure_column)).
pub struct World {
    pub seed: u32,
    /// Client/server role (see [`WorldRole`]); fixed at construction.
    pub(super) role: WorldRole,
    /// Loaded section voxel data. Private to the `world` module: every external
    /// mutation routes through an accessor (`set_block_world`, the dirty-mesh queue)
    /// so the queue stays the single source of truth for what needs remeshing.
    ///
    /// Stored behind `Arc` so the off-thread light and mesh pools can take a cheap shared
    /// handle to a section (and its neighbours) instead of the render thread deep-copying it
    /// per bake — assembling those neighbourhoods was a multi-millisecond per-frame spike
    /// while streaming. Mutation is copy-on-write via [`Arc::make_mut`]: a setter clones a
    /// section's storage only while a bake still holds the old handle.
    pub(super) sections: FxHashMap<SectionPos, Arc<Section>>,
    /// Per-column 2D data (biome, visible surface, direct-sky cover) shared by a
    /// vertical stack of sections. Cheap; ensured present whenever a section in
    /// the column loads.
    pub(super) columns: FxHashMap<ChunkPos, Column>,
    /// Per-column presentation revision (biome/surface/sky-cover/summaries).
    /// Terrain replication resends ColumnData only when this
    /// changes, and revision-gated surface sampling relies on EQUALITY:
    /// values come from `column_revision_counter`, so a value is never reused
    /// — not even by a column that unloads and reloads with other content.
    pub(super) column_payload_revisions: FxHashMap<ChunkPos, u64>,
    /// Store-wide source of unique column payload revision values.
    pub(super) column_revision_counter: u64,
    /// One GPU-ready mesh per section.
    pub(super) meshes: FxHashMap<SectionPos, ChunkMesh>,
    /// XZ columns that currently have at least one CPU section mesh.
    /// Mirrors `meshes` so renderer retention does not scan the vertical range
    /// of every GPU column each frame.
    pub(super) mesh_columns: FxHashSet<ChunkPos>,
    /// Changes whenever a section mesh enters or leaves a packed GPU column.
    /// The renderer uses it to coalesce consecutive sibling completions.
    pub(super) mesh_upload_revisions: FxHashMap<ChunkPos, u64>,
    /// XZ columns whose packed render buffer must be rebuilt from `meshes`.
    /// Kept explicitly so the renderer does not scan every section mesh each frame.
    pub(super) mesh_upload_dirty_columns: FxHashSet<ChunkPos>,
    /// Uploaded columns scheduled to release their CPU mesh buffers once they have
    /// been upload-quiet long enough (value = earliest release frame). The retained
    /// CPU copy exists only so a column repack can re-pack sibling sections; a
    /// settled column frees it and repacks force a remesh instead (`repack_forced`).
    pub(super) mesh_release_after: FxHashMap<ChunkPos, u64>,
    /// Released sections whose column needs a GPU repack: their remesh must not be
    /// skipped by deep-visibility parking — the packed column buffer cannot be
    /// rebuilt without their geometry.
    pub(super) repack_forced: FxHashSet<SectionPos>,
    /// Monotonic mesh-pump frame counter (drives `mesh_release_after`).
    pub(super) mesh_pump_frame: u64,
    pub worker: WorkerPool,
    /// Columns whose shared 2D gen data (`ColumnGen`) has landed: the source for
    /// submitting per-section jobs and sizing each column's vertical load window.
    /// Present for every loaded column; dropped when the column unloads.
    pub(super) column_gen: FxHashMap<ChunkPos, Arc<ColumnGen>>,
    /// Columns queued for the (heavy, once-per-column) `ColumnGen` job.
    pub(super) pending: FxHashMap<ChunkPos, Option<JobCancel>>,
    /// Sections with an in-flight per-section gen job, so the streamer never submits a
    /// section twice while it is being generated.
    pub(super) pending_sections: FxHashSet<SectionPos>,
    /// Cancellation handles for pending worker-generated sections. Disk-primary
    /// requests are in `pending_sections` without an entry here.
    pub(super) pending_section_jobs: FxHashMap<SectionPos, JobCancel>,
    /// Saved (player-modified) sections read back from disk whose generated column has
    /// not arrived yet — disk I/O usually beats noise-gen. Held here until the column
    /// lands, then overlaid over the generated terrain (see `world::stream::poll`).
    pub(super) pending_overlays: FxHashMap<SectionPos, super::stream::LoadedOverlay>,
    /// Sections whose saved record has been REQUESTED from the save thread but not
    /// answered yet. Until the answer lands (and any overlay applies) the section's
    /// true content is in flight: the sim guard blocks mutation and the harvest skips
    /// persisting it (see `world::sim_guard`).
    pub(super) awaited_overlays: FxHashSet<SectionPos>,
    /// Requested disk records that install as the section's PRIMARY content — no
    /// gen job was submitted for them ("Optimize explored terrain"). A corrupt
    /// answer falls back to generation; see `world::stream::submit_section_job`.
    pub(super) disk_primary_sections: FxHashSet<SectionPos>,
    pub render_dist: i32,
    pub(super) lighting_revision: u64,
    pub(super) light_bakes: LightBakeQueue,
    /// Asynchronous reconciliation light -> mesh bundles. Initial prediction
    /// runs the same complete invalidation footprint synchronously.
    pub(super) prediction_terrain: super::prediction_render::PredictionTerrainQueue,
    /// Ordinary off-thread section meshing: dirty sections are submitted as owned
    /// snapshots and finished meshes drained back. Local prediction deliberately
    /// invokes the same builder synchronously.
    pub(super) mesh_pool: super::mesh_pool::MeshPool,
    pub(super) mesh_jobs_in_flight: usize,
    /// Latest mesh job per section. Re-dirtying cancels queued stale work;
    /// completion tokens prevent an older result from clearing a newer handle.
    pub(super) mesh_job_cancels: FxHashMap<SectionPos, JobCancel>,
    pub(super) dirty_meshes: DirtyMeshQueue,
    /// Loaded sections wholly below their column's surface retention band — only
    /// visible through cave openings (see `world::visibility`).
    pub(super) deep_sections: FxHashSet<SectionPos>,
    /// The deep sections the last visibility refresh could reach from the visible
    /// region. Deep sections outside this set park instead of meshing.
    pub(super) visible_deep: FxHashSet<SectionPos>,
    /// Dirty deep sections parked because nothing can see them. Re-queued by the
    /// visibility refresh when they become reachable (or the player ring arrives).
    pub(super) hidden_parked: FxHashSet<SectionPos>,
    /// Dirty sections whose six exact loaded neighbour planes currently seal them
    /// from outside sightlines. Kept separate from deep visibility so a load-target
    /// move can wake them when a player may already be inside.
    pub(super) sealed_parked: FxHashSet<SectionPos>,
    /// Sections currently holding at least one chest, door, or furnace, so the
    /// per-frame chest/door collection and the furnace tick visit only those
    /// sections instead of scanning every loaded one (mirrors `mesh_columns`).
    /// Maintained by [`refresh_block_entity_index`](Self::refresh_block_entity_index)
    /// at every install/mutation point; may briefly over-approximate (an indexed
    /// section whose last entity was cleared by a raw block edit costs one
    /// `is_empty` check), never under-approximate.
    pub(super) block_entity_sections: FxHashSet<SectionPos>,
    /// Sections currently holding at least one block-row particle emitter. Kept separate
    /// from `block_entity_sections` so torch-heavy scenes do not make chest/door/furnace
    /// collection visit unrelated sections.
    pub(super) particle_emitter_sections: FxHashSet<SectionPos>,
    /// Raised by ingest / edits / load-target moves; consumed by the mesh pump,
    /// which re-runs the deep-visibility BFS before submitting work.
    pub(super) vis_dirty: bool,
    /// Dirty meshes parked while async light bakes their sampling neighbourhood.
    /// They re-enter `dirty_meshes` only once the 3×3×3 light dependency set is clean.
    pub(super) light_blocked_meshes: FxHashSet<SectionPos>,
    /// Freshly streamed sections that have never produced light or a mesh, parked
    /// until their generation neighbourhood settles (`gen_neighborhood_settled`) so
    /// their FIRST bake and mesh run once, not once per landing neighbour. Without
    /// this, contiguous streaming rebaked/remeshed each section many times (each
    /// ingest dirtied its whole 3×3×3).
    pub(super) light_deferred: FxHashSet<SectionPos>,
    /// A topology change may have made deferred first meshes ready. This keeps
    /// the O(deferred) settle scan off idle 200 Hz server pumps.
    pub(super) deferred_recheck_needed: bool,
    /// Deferred centres whose 3x3x3 dependency changed since their last check.
    /// Ordinary ingest drains only these; a target reshape uses the full flag.
    pub(super) deferred_rechecks: FxHashSet<SectionPos>,
    pub(super) last_load_target: Option<LoadTarget>,
    /// Anchors beyond the first under multi-anchor streaming
    /// ([`World::update_load_multi`]); empty in single-anchor mode, so every
    /// single-anchor path is byte-identical to before. `last_load_target`
    /// stays the PRIMARY anchor (the priority/fallback target).
    pub(super) extra_load_targets: Vec<LoadTarget>,
    /// The last missing-column scan found nothing left to request (everything
    /// wanted is loaded or pending), so the per-pump rescan can be skipped —
    /// with static anchors that scan is the entire steady-state streaming
    /// cost. Cleared by anything that can make a wanted column missing again:
    /// an anchor-set change, a column eviction, or a failed/discarded column
    /// gen job (see `poll_inner`).
    pub(super) missing_columns_settled: bool,
    /// Replica-only: each installed column's per-cy `SectionSummary`s from the
    /// server's `ColumnPayload`, indexed `cy - SECTION_MIN_CY`. Consulted by
    /// [`section_summary`](Self::section_summary) for ABSENT sections — the
    /// replica's stand-in for `column_gen`, so physics/placement answer
    /// truthfully without running worldgen. Empty on Combined/server worlds.
    pub(super) column_summaries: FxHashMap<ChunkPos, Box<[SectionSummary]>>,
    /// Replica-only tint halos and deep-band floors carried by ColumnPayload.
    /// Combined/server worlds read the same facts from `column_gen`.
    pub(super) column_biome_halos: FxHashMap<ChunkPos, Arc<[u8]>>,
    pub(super) column_deep_band_los: FxHashMap<ChunkPos, i32>,
    /// Server-side replication log gate; ~zero cost while off (one branch at
    /// the block-change choke point). See [`set_replication_capture`].
    ///
    /// [`set_replication_capture`]: Self::set_replication_capture
    pub(super) replication_capture: bool,
    /// This tick's coalesced block/water changes, latest state per cell —
    /// drained by [`take_block_deltas`](Self::take_block_deltas).
    pub(super) block_delta_log: FxHashMap<crate::mathh::IVec3, crate::net::protocol::BlockDelta>,
    /// Monotonic revision of "which sections exist / are stream-final": bumped
    /// on ingest, eviction, materialization, and in-flight-set changes. The
    /// per-connection terrain sender keys its wanted-vs-sent rescan on this
    /// (plus the anchor's quantized target), so a steady frame does no scan.
    pub(super) terrain_revision: u64,
    /// Sections whose light went dirty since the last
    /// [`pump_light_bakes`](Self::pump_light_bakes) drain: the mark choke
    /// point feeds this set and the light pump requests from it. This is the
    /// demand path that does not depend on any mesh being queued — a distant
    /// sky-cover segment relights without pre-marking meshes (the landed
    /// bake's diff decides those), and headless servers have no mesh pump at
    /// all. Nearby sections are usually ALSO demanded by the mesh pump's
    /// `request_light_dependencies`; the pending-bake dedup makes that free.
    /// Bounded by edits per tick.
    pub(super) relight_demand: FxHashSet<SectionPos>,
    /// ServerHeadless only: sections whose bake LANDED since the last
    /// streaming pump — drained by [`take_light_ship_log`](Self::take_light_ship_log)
    /// into per-connection `LightData` messages (filtered to each recipient's
    /// sent set). A set, so several bakes in one window ship latest-wins.
    pub(super) light_ship_log: FxHashSet<SectionPos>,
    /// Sections whose bake landed since the last save flush. Light changes
    /// don't set `modified` (they're derived, not player content), but a
    /// section whose on-disk record already exists must re-persist after a
    /// relight or its saved cubes go permanently stale (persisted light is
    /// only load-skippable because disk content is mutually consistent).
    /// Cleared wholesale by `flush_modified_chunks`. Empty without a save.
    pub(super) relit_since_persist: FxHashSet<SectionPos>,
    /// Sections whose baked light a CONTENT change dirtied while a save is
    /// attached — their on-disk cubes are now pre-edit stale. Resolved by the
    /// rebake landing (`pump_light_bakes` moves them to `relit_since_persist`)
    /// or, if eviction/quit wins the race, by the persist gate rewriting the
    /// record WITHOUT light so reload rebakes instead of loading a permanent
    /// dark seam. Streaming-landing dirt is deliberately NOT tracked: those
    /// records stay mutually consistent on disk.
    pub(super) light_edited_since_persist: FxHashSet<SectionPos>,
    /// Fixed-timestep simulation state: block updates + scheduled block ticks.
    pub(super) sim: TickState,
    /// On-disk save handle (`None` if saving is disabled / failed to open).
    pub(super) save: Option<WorldSave>,
    /// The world's "Optimize explored terrain" setting: persist EVERY explored
    /// section (not just modified ones) plus the per-column gen cache, so
    /// revisited terrain loads from disk instead of regenerating. Set once at
    /// session open from `settings.json`; meaningless without a save.
    pub(super) optimize_explored_terrain: bool,
    /// Column-gen cache records awaiting a batched write ("Optimize explored
    /// terrain"): buffered so the save thread merges many columns per region
    /// file rewrite instead of read-modify-writing per column. Records are
    /// pure gen data — a crash losing the buffer only costs a future regen.
    pub(super) pending_colgen_records: Vec<crate::save::colgen::ColumnGenRecord>,
    /// Active dropped item entities resting in currently-loaded sections.
    pub(super) dropped_items: DroppedItems,
    /// Active mobs in currently-loaded sections.
    pub(super) mobs: Mobs,
    /// Player-on-mob riding attachments (see `mob::riding`). On `World` so the
    /// mount HostCalls reach it through `SimCtx`; the server's riding pass
    /// reconciles sessions against it each tick. Never persisted.
    pub(super) riding: crate::mob::riding::Riding,
    /// Every connected player's movement intent this tick, decomposed into
    /// its own yaw frame (see [`crate::player::PlayerInputSnapshot`]) —
    /// published by the server before the tick stages so the `PlayerInput`
    /// HostCall can answer from the world. Replaced wholesale each tick.
    pub(super) player_inputs: Vec<crate::player::PlayerInputSnapshot>,
    /// Behavior hooks fired on mod-behavior blocks this tick (see
    /// `block::behavior::wasm`), in fire order. Drained by the game right
    /// after the world tick and dispatched to the owning mods; only blocks
    /// whose rows declare a `mod_id:name` behavior ever enqueue here.
    pub(super) mod_block_hooks: Vec<crate::block::behavior::ModBlockHook>,
    /// Section installs the per-frame streamer buffered for the tick-side event bus
    /// (`section_generated` / `section_loaded`); drained by the next game tick.
    pub(super) stream_events: Vec<super::stream::StreamEvent>,
    /// Buffer gate, mirroring event-bus listener presence (set once per tick), so
    /// streaming costs nothing while nothing listens.
    pub(super) stream_events_enabled: bool,
    /// Sim-owned visual shader parameters.
    /// Mutated on the tick only (mod HostCalls); NOT persisted — resets to
    /// defaults on world open, the owning mod re-applies it (Phase 3 world KV).
    pub(super) environment: WorldEnvironment,
    /// Persistent mod world KV (`mod_id:key` → bytes) — the cross-mod interop
    /// surface (Phase 3b). BTreeMap so the save encoding (it
    /// rides `level.dat`) iterates in one deterministic order. Mutated on the
    /// tick only (mod HostCalls); restored at session open.
    pub(super) mod_kv: BTreeMap<String, Vec<u8>>,
    /// Mod pack ids DISABLED for this world (per-world `settings.json`; empty
    /// = all enabled). Session-fixed, set once at open; the natural spawner
    /// and the mod-set record consult it. The palette/mod-host gates take it
    /// separately at session construction.
    pub(super) disabled_mods: std::collections::BTreeSet<String>,
    /// Chunk columns whose one-time worldgen herd actually spawned (see
    /// `mob::populate`) — the fact that keeps the initial animal stock from
    /// re-minting every session. Persisted in `level.dat`; BTreeSet so the
    /// encoding iterates in one deterministic order. Mutated on the tick only.
    pub(super) populated_columns: BTreeSet<ChunkPos>,
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
    /// local client's replica can share one pool instead of each spawning a
    /// machine-sized thread set (Phase C runs both in one process).
    pub fn new_with_pool(
        seed: u32,
        render_dist: i32,
        role: WorldRole,
        jobs: std::sync::Arc<JobPool>,
    ) -> Self {
        Self {
            seed,
            role,
            sections: FxHashMap::default(),
            columns: FxHashMap::default(),
            column_payload_revisions: FxHashMap::default(),
            column_revision_counter: 0,
            meshes: FxHashMap::default(),
            mesh_columns: FxHashSet::default(),
            mesh_upload_revisions: FxHashMap::default(),
            mesh_upload_dirty_columns: FxHashSet::default(),
            mesh_release_after: FxHashMap::default(),
            repack_forced: FxHashSet::default(),
            mesh_pump_frame: 0,
            worker: WorkerPool::new(jobs.clone()),
            column_gen: FxHashMap::default(),
            pending: FxHashMap::default(),
            pending_sections: FxHashSet::default(),
            pending_section_jobs: FxHashMap::default(),
            pending_overlays: FxHashMap::default(),
            awaited_overlays: FxHashSet::default(),
            disk_primary_sections: FxHashSet::default(),
            render_dist,
            lighting_revision: 0,
            light_bakes: LightBakeQueue::new(jobs.clone()),
            prediction_terrain: super::prediction_render::PredictionTerrainQueue::new(jobs.clone()),
            mesh_pool: super::mesh_pool::MeshPool::new(jobs),
            mesh_jobs_in_flight: 0,
            mesh_job_cancels: FxHashMap::default(),
            dirty_meshes: DirtyMeshQueue::default(),
            deep_sections: FxHashSet::default(),
            visible_deep: FxHashSet::default(),
            hidden_parked: FxHashSet::default(),
            sealed_parked: FxHashSet::default(),
            block_entity_sections: FxHashSet::default(),
            particle_emitter_sections: FxHashSet::default(),
            vis_dirty: false,
            light_blocked_meshes: FxHashSet::default(),
            light_deferred: FxHashSet::default(),
            deferred_recheck_needed: false,
            deferred_rechecks: FxHashSet::default(),
            last_load_target: None,
            extra_load_targets: Vec::new(),
            missing_columns_settled: false,
            column_summaries: FxHashMap::default(),
            column_biome_halos: FxHashMap::default(),
            column_deep_band_los: FxHashMap::default(),
            replication_capture: false,
            block_delta_log: FxHashMap::default(),
            terrain_revision: 0,
            relight_demand: FxHashSet::default(),
            light_ship_log: FxHashSet::default(),
            relit_since_persist: FxHashSet::default(),
            light_edited_since_persist: FxHashSet::default(),
            sim: TickState::new(seed),
            save: None,
            optimize_explored_terrain: false,
            pending_colgen_records: Vec::new(),
            dropped_items: DroppedItems::default(),
            mobs: Mobs::new(seed as u64),
            riding: Default::default(),
            player_inputs: Vec::new(),
            mod_block_hooks: Vec::new(),
            stream_events: Vec::new(),
            stream_events_enabled: false,
            environment: WorldEnvironment::default(),
            mod_kv: BTreeMap::new(),
            disabled_mods: std::collections::BTreeSet::new(),
            populated_columns: BTreeSet::new(),
        }
    }

    /// Mod pack ids disabled for this world (per-world `settings.json`).
    #[inline]
    pub fn disabled_mods(&self) -> &std::collections::BTreeSet<String> {
        &self.disabled_mods
    }

    /// Install the world's disabled-mod set — once, at session open.
    pub fn set_disabled_mods(&mut self, disabled: std::collections::BTreeSet<String>) {
        self.disabled_mods = disabled;
    }

    /// The sim-owned visual shader parameter state (see [`WorldEnvironment`]).
    pub fn environment(&self) -> &WorldEnvironment {
        &self.environment
    }

    /// Set one namespaced visual shader parameter. Tick-side only; not persisted
    /// by the engine, so the owning mod should re-apply it from its own state.
    pub fn set_shader_param(&mut self, key: String, value: [f32; 4]) {
        self.environment.set_shader_param(key, value);
    }

    /// Client/server role, fixed at construction.
    #[inline]
    pub fn role(&self) -> WorldRole {
        self.role
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
        self.vis_dirty = true;
    }

    /// Replace the published per-player input snapshots for this tick (see
    /// [`crate::player::PlayerInputSnapshot`]).
    pub fn set_player_inputs(&mut self, inputs: Vec<crate::player::PlayerInputSnapshot>) {
        self.player_inputs = inputs;
    }

    /// The published input snapshot for `player`, if connected this tick.
    pub fn player_input(&self, player: u8) -> Option<crate::player::PlayerInputSnapshot> {
        self.player_inputs.iter().find(|i| i.id == player).copied()
    }

    #[inline]
    pub fn lighting_revision(&self) -> u64 {
        self.lighting_revision
    }

    pub(super) fn bump_lighting_revision(&mut self) {
        self.lighting_revision = self.lighting_revision.wrapping_add(1);
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
                .column_gen
                .get(&pos.chunk_pos())
                .filter(|col| col.section_summary(pos.cy) != SectionSummary::Empty)
                .map(|col| ChunkGenerator::new(self.seed).generate_section(pos, col))
                .unwrap_or_else(|| Section::new(pos.cx, pos.cy, pos.cz));
            self.ensure_column(pos.chunk_pos());
            self.sections.insert(pos, Arc::new(section));
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
    pub(super) fn bump_terrain_revision(&mut self) {
        self.terrain_revision = self.terrain_revision.wrapping_add(1);
    }

    /// [`materialize_section`](Self::materialize_section) for the section owning world
    /// cell `c`. Returns `false` if `c` is outside the world vertical range.
    pub(super) fn materialize_section_at(&mut self, c: crate::mathh::IVec3) -> bool {
        match SectionPos::from_world(c.x, c.y, c.z) {
            Some(sp) => self.materialize_section(sp),
            None => false,
        }
    }

    // --- Column data ------------------------------------------------------------

    /// Ensure the per-column data for `(cx,cz)` exists, building it cheaply if not.
    /// Worldgen fills biome + both height maps; an empty column is the pre-gen placeholder.
    pub(super) fn ensure_column(&mut self, pos: ChunkPos) -> &mut Column {
        if !self.column_payload_revisions.contains_key(&pos) {
            self.column_revision_counter += 1;
            self.column_payload_revisions
                .insert(pos, self.column_revision_counter);
        }
        self.columns.entry(pos).or_insert_with(Column::new)
    }

    pub(super) fn bump_column_payload_revision(&mut self, pos: ChunkPos) {
        self.column_revision_counter += 1;
        self.column_payload_revisions
            .insert(pos, self.column_revision_counter);
    }

    pub(crate) fn column_payload_revision(&self, pos: ChunkPos) -> u64 {
        self.column_payload_revisions
            .get(&pos)
            .copied()
            .unwrap_or(0)
    }

    #[inline]
    pub(super) fn column_at(&self, wx: i32, wz: i32) -> Option<&Column> {
        self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))
    }

    // --- World-coordinate routing ----------------------------------------------

    /// The one world-coordinate router: decode a world voxel `(wx, wy, wz)` into its
    /// owning [`SectionPos`] and section-local coords `(lx, ly, lz)` (each `0..16`),
    /// or `None` when `wy` falls outside the world vertical range. Section lookup is
    /// a separate step (see [`chunk_at_world`](Self::chunk_at_world)).
    #[inline]
    pub(super) fn split_world(
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(SectionPos, usize, usize, usize)> {
        let sp = SectionPos::from_world(wx, wy, wz)?;
        Some((
            sp,
            chunk::lx(wx),
            wy.rem_euclid(SECTION_SIZE as i32) as usize,
            chunk::lz(wz),
        ))
    }

    /// The loaded section owning world voxel `(wx, wy, wz)` plus its section-local
    /// coords, or `None` if `wy` is out of range or the section is not loaded. The
    /// shared front end for every read-side world-coordinate accessor. (Named
    /// `chunk_at_world` for continuity; the unit it returns is now a [`Section`].)
    #[inline]
    pub(super) fn chunk_at_world(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(&Section, usize, usize, usize)> {
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let s = self.sections.get(&pos)?;
        Some((s, lx, ly, lz))
    }

    /// Mutable counterpart of [`chunk_at_world`](Self::chunk_at_world).
    #[inline]
    pub(super) fn chunk_at_world_mut(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(&mut Section, usize, usize, usize)> {
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let s = self.section_mut(pos)?;
        Some((s, lx, ly, lz))
    }

    #[inline]
    pub(super) fn section_mut(&mut self, pos: SectionPos) -> Option<&mut Section> {
        self.sections.get_mut(&pos).map(Arc::make_mut)
    }

    /// Whether the section owning world `(wx,wy,wz)` is loaded.
    #[inline]
    pub fn section_loaded_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        SectionPos::from_world(wx, wy, wz).is_some_and(|p| self.sections.contains_key(&p))
    }

    /// Cheap occupancy fact for a section, even when the voxel buffer has not been
    /// materialized. Loaded sections answer from exact counters. Unloaded generated
    /// sections answer from their column's surface/content summary, unless a saved overlay
    /// could replace the generated base.
    pub(super) fn section_summary(&self, pos: SectionPos) -> SectionSummary {
        if !SectionPos::cy_in_range(pos.cy) {
            return SectionSummary::Unknown;
        }
        if let Some(section) = self.sections.get(&pos) {
            return section.summary();
        }
        if self.saved_section_contains(pos) {
            return SectionSummary::Unknown;
        }
        if let Some(col) = self.column_gen.get(&pos.chunk_pos()) {
            return col.section_summary(pos.cy);
        }
        // Replica: an absent section answers from the server's ColumnPayload
        // summaries — the wire stand-in for generated column facts.
        if let Some(sums) = self.column_summaries.get(&pos.chunk_pos()) {
            let idx = (pos.cy - SECTION_MIN_CY) as usize;
            return sums.get(idx).copied().unwrap_or(SectionSummary::Unknown);
        }
        SectionSummary::Unknown
    }

    /// Exact block when loaded, otherwise a conservative generated-summary placeholder
    /// for broad physics and AI probes. This is NOT an editing/readback API: mixed or
    /// unknown absent sections still read as air here so unloaded terrain does not become
    /// an invisible wall.
    pub fn physics_block(&self, wx: i32, wy: i32, wz: i32) -> Block {
        if let Some((section, lx, ly, lz)) = self.chunk_at_world(wx, wy, wz) {
            return section.block(lx, ly, lz);
        }
        let Some(pos) = SectionPos::from_world(wx, wy, wz) else {
            return Block::Air;
        };
        self.section_summary(pos).virtual_block()
    }

    #[inline]
    pub fn blocks_movement_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        self.physics_block(wx, wy, wz).blocks_movement()
    }

    #[inline]
    pub fn water_cell_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        self.physics_block(wx, wy, wz) == Block::Water
    }

    #[inline]
    pub(super) fn column_has_mesh(&self, pos: ChunkPos) -> bool {
        self.mesh_columns.contains(&pos)
    }

    pub(super) fn install_mesh(&mut self, pos: SectionPos, mesh: ChunkMesh) {
        self.meshes.insert(pos, mesh);
        self.repack_forced.remove(&pos);
        let column = pos.chunk_pos();
        self.mesh_columns.insert(column);
        self.bump_mesh_upload_revision(column);
        self.mesh_upload_dirty_columns.insert(column);
    }

    pub(super) fn remove_mesh(&mut self, pos: SectionPos) -> bool {
        let removed = self.meshes.remove(&pos).is_some();
        self.repack_forced.remove(&pos);
        if removed {
            let column = pos.chunk_pos();
            self.refresh_mesh_column_presence(column);
            self.bump_mesh_upload_revision(column);
        }
        removed
    }

    fn bump_mesh_upload_revision(&mut self, pos: ChunkPos) {
        let revision = self.mesh_upload_revisions.entry(pos).or_insert(0);
        *revision = revision.wrapping_add(1).max(1);
    }

    fn refresh_mesh_column_presence(&mut self, pos: ChunkPos) {
        let has_mesh = Self::column_section_range().any(|cy| {
            self.meshes
                .contains_key(&SectionPos::new(pos.cx, cy, pos.cz))
        });
        if has_mesh {
            self.mesh_columns.insert(pos);
        } else {
            self.mesh_columns.remove(&pos);
        }
    }

    /// Mark the section owning world voxel `pos` as modified, so a change that no
    /// tick would otherwise re-flag (a GUI edit to an idle chest/furnace) persists.
    pub fn mark_chunk_modified(&mut self, pos: crate::mathh::IVec3) {
        if let Some((s, ..)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            s.modified = true;
        }
    }

    /// Queue a mod-behavior hook for post-tick dispatch (called by
    /// `block::behavior::wasm`'s hooks, on the tick only).
    pub fn queue_mod_block_hook(&mut self, hook: crate::block::behavior::ModBlockHook) {
        self.mod_block_hooks.push(hook);
    }

    /// Drain the mod-behavior hooks fired this tick, in fire order.
    pub fn take_mod_block_hooks(&mut self) -> Vec<crate::block::behavior::ModBlockHook> {
        std::mem::take(&mut self.mod_block_hooks)
    }

    /// [`refresh_block_entity_index`](Self::refresh_block_entity_index) for the
    /// section owning world cell `pos`.
    pub(super) fn note_block_entity_change(&mut self, pos: crate::mathh::IVec3) {
        if let Some(sp) = SectionPos::from_world(pos.x, pos.y, pos.z) {
            self.refresh_block_entity_index(sp);
        }
    }

    /// Keep [`block_entity_sections`](Self::block_entity_sections) in sync after
    /// `pos`'s content may have changed (section install, container/door/furnace
    /// insert or removal).
    pub(super) fn refresh_block_entity_index(&mut self, pos: SectionPos) {
        let has = self.sections.get(&pos).is_some_and(|s| {
            !s.containers().is_empty()
                || !s.doors().is_empty()
                || !s.furnaces().is_empty()
                || !s.entity_facings().is_empty()
        });
        if has {
            self.block_entity_sections.insert(pos);
        } else {
            self.block_entity_sections.remove(&pos);
        }
    }

    /// Keep [`particle_emitter_sections`](Self::particle_emitter_sections) in sync after
    /// `pos`'s block ids may have changed.
    pub(super) fn refresh_particle_emitter_index(&mut self, pos: SectionPos) {
        let has = self
            .sections
            .get(&pos)
            .is_some_and(|s| s.has_particle_emitters());
        if has {
            self.particle_emitter_sections.insert(pos);
        } else {
            self.particle_emitter_sections.remove(&pos);
        }
    }

    pub(super) fn remove_section(&mut self, pos: SectionPos) {
        self.prediction_terrain.cancel_section(pos);
        if let Some(job) = self.mesh_job_cancels.remove(&pos) {
            job.cancel();
        }
        if let Some(job) = self.pending_section_jobs.remove(&pos) {
            job.cancel();
        }
        self.pending_sections.remove(&pos);
        let section_removed = self.sections.remove(&pos).is_some();
        if section_removed {
            self.bump_column_payload_revision(pos.chunk_pos());
        }
        self.block_entity_sections.remove(&pos);
        self.particle_emitter_sections.remove(&pos);
        self.awaited_overlays.remove(&pos);
        self.disk_primary_sections.remove(&pos);
        if self.remove_mesh(pos) {
            self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
        }
        self.dirty_meshes.remove(pos);
        self.light_blocked_meshes.remove(&pos);
        self.light_deferred.remove(&pos);
        self.deferred_rechecks.remove(&pos);
        self.deep_sections.remove(&pos);
        self.visible_deep.remove(&pos);
        self.hidden_parked.remove(&pos);
        self.sealed_parked.remove(&pos);
        self.light_bakes.cancel(pos);
        self.light_edited_since_persist.remove(&pos);
        self.mark_light_dirty_neighborhood(pos, false);
        self.mark_dirty_neighborhood(pos, false);
    }

    /// Evict an entire column: all its loaded sections, meshes, queues, per-column data,
    /// and any pending gen.
    pub(super) fn remove_column(&mut self, pos: ChunkPos) {
        // An evicted column is missing again if an anchor still wants it —
        // the settled short-circuit must not hide it from the next scan.
        self.missing_columns_settled = false;
        for cy in Self::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.prediction_terrain.cancel_section(sp);
            self.sections.remove(&sp);
            self.block_entity_sections.remove(&sp);
            self.particle_emitter_sections.remove(&sp);
            self.meshes.remove(&sp);
            if let Some(job) = self.mesh_job_cancels.remove(&sp) {
                job.cancel();
            }
            self.repack_forced.remove(&sp);
            self.dirty_meshes.remove(sp);
            self.light_blocked_meshes.remove(&sp);
            self.light_deferred.remove(&sp);
            self.deferred_rechecks.remove(&sp);
            self.deep_sections.remove(&sp);
            self.visible_deep.remove(&sp);
            self.hidden_parked.remove(&sp);
            self.sealed_parked.remove(&sp);
            self.light_bakes.cancel(sp);
            self.light_edited_since_persist.remove(&sp);
        }
        self.mesh_columns.remove(&pos);
        self.mesh_upload_revisions.remove(&pos);
        self.mesh_upload_dirty_columns.remove(&pos);
        self.mesh_release_after.remove(&pos);
        self.columns.remove(&pos);
        self.column_payload_revisions.remove(&pos);
        self.column_gen.remove(&pos);
        self.column_summaries.remove(&pos);
        self.column_biome_halos.remove(&pos);
        self.column_deep_band_los.remove(&pos);
        if let Some(Some(job)) = self.pending.remove(&pos) {
            job.cancel();
        }
        let section_jobs: Vec<_> = self
            .pending_section_jobs
            .keys()
            .filter(|sp| sp.chunk_pos() == pos)
            .copied()
            .collect();
        for sp in section_jobs {
            if let Some(job) = self.pending_section_jobs.remove(&sp) {
                job.cancel();
            }
        }
        self.pending_sections.retain(|sp| sp.chunk_pos() != pos);
        self.awaited_overlays.retain(|sp| sp.chunk_pos() != pos);
        self.disk_primary_sections
            .retain(|sp| sp.chunk_pos() != pos);
    }

    /// Drop all loaded sections, columns, meshes, and the in-flight gen set — the
    /// regen path.
    pub fn clear_world(&mut self) {
        self.prediction_terrain.cancel_all();
        self.sections.clear();
        self.deep_sections.clear();
        self.visible_deep.clear();
        self.hidden_parked.clear();
        self.sealed_parked.clear();
        self.block_entity_sections.clear();
        self.particle_emitter_sections.clear();
        self.columns.clear();
        self.column_payload_revisions.clear();
        self.column_gen.clear();
        self.column_summaries.clear();
        self.column_biome_halos.clear();
        self.column_deep_band_los.clear();
        self.meshes.clear();
        for job in self.mesh_job_cancels.values() {
            job.cancel();
        }
        self.mesh_job_cancels.clear();
        self.mesh_columns.clear();
        self.mesh_upload_revisions.clear();
        self.mesh_upload_dirty_columns.clear();
        self.mesh_release_after.clear();
        self.repack_forced.clear();
        self.light_blocked_meshes.clear();
        self.light_deferred.clear();
        self.light_edited_since_persist.clear();
        self.deferred_recheck_needed = false;
        self.deferred_rechecks.clear();
        for job in self.pending.values().flatten() {
            job.cancel();
        }
        self.pending.clear();
        for job in self.pending_section_jobs.values() {
            job.cancel();
        }
        self.pending_section_jobs.clear();
        self.pending_sections.clear();
        self.pending_overlays.clear();
        self.awaited_overlays.clear();
        self.disk_primary_sections.clear();
        self.bump_terrain_revision();
    }

    /// All section coordinates of column `(cx,cz)` in the world vertical range.
    /// Concrete `RangeInclusive` (not `impl Iterator`) so callers can `.rev()` it.
    pub(super) fn column_section_range() -> std::ops::RangeInclusive<i32> {
        SECTION_MIN_CY..=SECTION_MAX_CY
    }

    /// Install a section for a test, mirroring the streamer's per-section install.
    #[cfg(test)]
    pub(crate) fn insert_section_for_test(&mut self, pos: SectionPos, section: Section) {
        self.ensure_column(pos.chunk_pos());
        self.sections.insert(pos, Arc::new(section));
        self.refresh_block_entity_index(pos);
        self.refresh_particle_emitter_index(pos);
        self.queue_dirty_mesh(pos);
        self.request_fixture_bake(pos);
        self.bump_terrain_revision();
    }

    /// Fixture sections bypass the streamer's settle/defer path, so a headless
    /// world would never bake them — and the light-final ship gate would hold
    /// them back forever. Feed the relight queue like an edit does.
    #[cfg(test)]
    fn request_fixture_bake(&mut self, pos: SectionPos) {
        self.relight_demand.insert(pos);
    }

    /// Install a whole column [`Chunk`] for a test, splitting it into sections + column
    /// data exactly as the streamer does for a generated column. Lets the many column-era
    /// fixtures (which build a 256-tall `Chunk` and hand it over) keep working against the
    /// cubic store unchanged. `pos` must match the chunk's own `(cx,cz)`.
    ///
    /// Carries blocks + water + biome + heightmap, but NOT block-entities (furnaces,
    /// chests, …) — matching real worldgen, which produces none. A test that needs a
    /// block-entity should build the [`Section`] directly (with `insert_section_for_test`)
    /// or place it through the world API after install.
    #[cfg(test)]
    pub(crate) fn insert_chunk_for_test(&mut self, pos: ChunkPos, chunk: crate::chunk::Chunk) {
        debug_assert_eq!((pos.cx, pos.cz), (chunk.cx, chunk.cz));
        let (column, sections) = super::stream::split_generated_column(&chunk);
        self.columns.insert(pos, column);
        // A test chunk is fully known, so record per-section summaries: its
        // absent sections are genuinely empty sky, and probes that consult
        // `section_summary` (sapling growth validation, physics) read them as
        // air — matching what a generated column's facts would answer.
        let mut sums = vec![SectionSummary::Empty; (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize]
            .into_boxed_slice();
        for (cy, section) in sections {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            sums[(cy - SECTION_MIN_CY) as usize] = section.summary();
            self.sections.insert(sp, Arc::new(section));
            self.refresh_particle_emitter_index(sp);
            self.queue_dirty_mesh(sp);
            self.request_fixture_bake(sp);
        }
        self.column_summaries.insert(pos, sums);
        self.bump_terrain_revision();
    }

    /// Arm the random-tick / streaming anchor for a test world. Random ticks
    /// only run around a load target, which production code sets from the
    /// per-frame streaming path a direct-stepping test never exercises.
    #[cfg(test)]
    pub(crate) fn set_load_target_for_test(&mut self, cx: i32, cy: i32, cz: i32, render_dist: i32) {
        self.last_load_target = Some(LoadTarget::new(cx, cy, cz, render_dist));
    }

    /// Install an entire column of empty (all-air) sections for a test, so
    /// world-coordinate edits anywhere in the vertical range land in a loaded section.
    /// Unlike [`insert_chunk_for_test`](Self::insert_chunk_for_test) with an empty
    /// `Chunk` (whose all-air surface sections would be skipped), this keeps every
    /// section present — the cubic analogue of the column era's "one empty loaded chunk".
    #[cfg(test)]
    pub(crate) fn insert_empty_column_for_test(&mut self, pos: ChunkPos) {
        self.ensure_column(pos);
        for cy in Self::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.sections
                .insert(sp, Arc::new(Section::new(pos.cx, cy, pos.cz)));
            self.refresh_particle_emitter_index(sp);
            self.queue_dirty_mesh(sp);
            self.request_fixture_bake(sp);
        }
        self.bump_terrain_revision();
    }

    /// The loaded section owning world voxel `(wx,wy,wz)`, for a test that inspects
    /// per-section light/flags after a world-coordinate edit.
    #[cfg(test)]
    pub(crate) fn section_at_world_for_test(&self, wx: i32, wy: i32, wz: i32) -> Option<&Section> {
        let pos = SectionPos::from_world(wx, wy, wz)?;
        self.sections.get(&pos).map(|s| &**s)
    }

    /// Mutable counterpart of [`section_at_world_for_test`](Self::section_at_world_for_test).
    #[cfg(test)]
    pub(crate) fn section_at_world_mut_for_test(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<&mut Section> {
        let pos = SectionPos::from_world(wx, wy, wz)?;
        self.section_mut(pos)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::block::Block;
    use crate::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY, SECTION_SIZE, SECTION_VOLUME};
    use crate::mathh::IVec3;
    use crate::mesh::ChunkMesh;
    use crate::section::Section;
    use crate::worldgen::driver::ChunkGenerator;

    use super::World;

    fn install_column_summary(world: &mut World, generator: &ChunkGenerator, pos: ChunkPos) {
        world.ensure_column(pos);
        world
            .column_gen
            .insert(pos, Arc::new(generator.generate_column_gen(pos.cx, pos.cz)));
    }

    #[test]
    fn same_height_surface_swap_bumps_the_column_revision() {
        // Replacing the visible surface block in place (tilling, grass
        // spread, …) keeps the heightmap, but revision-gated surface
        // sampling must still see the column move or the swapped color is
        // never resampled.
        let mut world = World::new(0, 0);
        let sp = SectionPos::new(0, 4, 0);
        let mut s = Section::new(0, 4, 0);
        s.set_block(8, 0, 8, Block::Stone);
        world.insert_section_for_test(sp, s);
        let column = world.ensure_column(sp.chunk_pos());
        column.set_surface_y(8, 8, 64);
        column.set_sky_cover_y(8, 8, 64);

        let before = world.column_payload_revision(sp.chunk_pos());
        world.set_block_world(8, 64, 8, Block::Dirt);
        assert_eq!(
            world.columns[&sp.chunk_pos()].surface_y(8, 8),
            64,
            "fixture: the swap must not move the heightmap"
        );
        assert_ne!(
            before,
            world.column_payload_revision(sp.chunk_pos()),
            "a same-height surface swap must move the column revision"
        );
    }

    #[test]
    fn edits_in_total_darkness_skip_light_invalidation_entirely() {
        // The adaptive relight radius: light values bound how far a plain
        // solid⇄air edit can matter, so mining inside unlit solid rock (the
        // hot gameplay path) must trigger NO light invalidation or rebake.
        let mut world = World::new(0, 4);
        let pos = SectionPos::new(0, 0, 0);
        let mut section = Section::new(0, 0, 0);
        section.blocks_slice_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        world.insert_section_for_test(pos, section);
        {
            let s = world.section_mut(pos).unwrap();
            s.set_skylight(vec![0u8; SECTION_VOLUME].into());
            s.set_blocklight(vec![0u8; SECTION_VOLUME].into());
        }
        // The fixture insert demands a bake; only the edits below are under test.
        world.relight_demand.clear();
        assert!(!world.sections[&pos].light_dirty, "fixture: settled dark");

        assert!(world.set_block_world(8, 8, 8, Block::Air));
        assert!(
            !world.sections[&pos].light_dirty,
            "no light can reach the opened cell, so nothing may invalidate"
        );
        assert!(world.relight_demand.is_empty());

        // Control: the same break beside cached light must invalidate.
        world
            .section_mut(pos)
            .unwrap()
            .set_skylight(vec![crate::chunk::SKY_FULL; SECTION_VOLUME].into());
        assert!(world.set_block_world(8, 4, 8, Block::Air));
        assert!(
            world.sections[&pos].light_dirty,
            "a break beside lit cells must invalidate light"
        );
        assert!(world.relight_demand.contains(&pos));
    }

    #[test]
    fn glass_raises_the_visible_surface_without_raising_sky_cover() {
        let mut world = World::new(0, 0);
        let cp = ChunkPos::new(0, 0);

        assert!(world.set_block_world(8, 0, 8, Block::Stone));
        assert!(world.set_block_world(8, 64, 8, Block::Glass));

        let column = &world.columns[&cp];
        assert_eq!(column.surface_y(8, 8), 64);
        assert_eq!(
            column.sky_cover_y(8, 8),
            0,
            "clear glass must not hide the open shaft from the skylight planner"
        );
    }

    #[test]
    fn eviction_racing_an_edit_relight_rewrites_the_record_lightless() {
        // Two adjacent sections persist with clean baked light. An edit in A
        // then dirties B's light (content change → B's on-disk cubes are
        // pre-edit stale). If eviction/quit wins the race against B's rebake,
        // the persist gate must rewrite B's record WITHOUT light so reload
        // rebakes — the pre-fix gate skipped unmodified light-dirty sections
        // entirely, stranding the stale cubes as a permanent dark seam.
        let dir =
            std::env::temp_dir().join(format!("petramond-stale-light-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let opened = crate::save::open_at(dir.clone()).expect("open save");
        let mut world = World::new(0, 0);
        world.attach_save(opened.save);

        let a = SectionPos::new(0, 4, 0);
        let b = SectionPos::new(1, 4, 0);
        for &sp in &[a, b] {
            let mut s = Section::new(sp.cx, sp.cy, sp.cz);
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    s.set_block(x, 0, z, Block::Stone);
                }
            }
            s.set_skylight(vec![0u8; SECTION_VOLUME].into());
            s.set_blocklight(vec![0u8; SECTION_VOLUME].into());
            s.mark_light_clean();
            world.insert_section_for_test(sp, s);
            world.section_mut(sp).expect("loaded").modified = true;
        }
        world.flush_modified_chunks();
        assert!(
            world.save().expect("save").manifest_contains(b),
            "fixture: B's record is on disk"
        );
        assert!(
            !world.sections[&b].light_dirty,
            "fixture: B persisted with clean light"
        );

        // The edit in A, one cell from the seam: B's cached AND persisted
        // light are now stale.
        world.set_block_world(15, 65, 8, Block::Stone);
        assert!(world.sections[&b].light_dirty);

        let snap = world
            .snapshot_section_for_save(b, Vec::new(), Vec::new(), false)
            .expect("an unmodified on-disk section with edit-dirtied light must rewrite");
        assert!(
            snap.skylight.is_none() && snap.blocklight.is_none(),
            "the rewrite must omit the stale cubes so reload rebakes"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mesh_column_index_tracks_multiple_vertical_meshes() {
        let mut world = World::new(0, 0);
        let lower = SectionPos::new(4, 0, -2);
        let upper = SectionPos::new(4, 1, -2);
        let column = lower.chunk_pos();

        assert!(!world.column_has_mesh(column));
        world.install_mesh(lower, ChunkMesh::empty());
        world.install_mesh(upper, ChunkMesh::empty());
        assert!(world.column_has_mesh(column));

        assert!(world.remove_mesh(lower));
        assert!(world.column_has_mesh(column));

        assert!(world.remove_mesh(upper));
        assert!(!world.column_has_mesh(column));
    }

    #[test]
    fn virtual_full_opaque_summary_blocks_collision_without_raw_voxels() {
        let seed = 0x51EED;
        let generator = ChunkGenerator::new(seed);
        let mut world = World::new(seed, 0);
        install_column_summary(&mut world, &generator, ChunkPos::new(0, 0));

        let y = SECTION_MIN_CY * SECTION_SIZE as i32;
        assert_eq!(
            Block::from_id(world.chunk_block(0, y, 0)),
            Block::Air,
            "raw reads stay exact: absent voxel buffers still read as air"
        );
        assert_eq!(
            world.physics_block(0, y, 0),
            Block::Stone,
            "physics reads may use the generated full-opaque summary"
        );
        assert!(
            !world.collision_boxes_at(0, y, 0).is_empty(),
            "virtual full-opaque summary should collide as a full block"
        );
        assert!(
            !world.placement_cell_open(IVec3::new(0, y, 0)),
            "placement must not treat absent known-solid terrain as open air"
        );
    }

    #[test]
    fn heightmap_recompute_preserves_generated_cave_mouth_surface() {
        let seed = 0x1234_5678;
        let generator = ChunkGenerator::new(seed);
        let mut found = None;

        'search: for cz in -8..=8 {
            for cx in -8..=8 {
                let col = Arc::new(generator.generate_column_gen(cx, cz));
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let original = col.surface_y(x, z);
                        let cave_top = col.heightmap_surface_y(x, z);
                        if cave_top < original {
                            found = Some((ChunkPos::new(cx, cz), col, x, z, original, cave_top));
                            break 'search;
                        }
                    }
                }
            }
        }

        let Some((cp, col, x, z, original, cave_top)) = found else {
            panic!("test seed/search window must contain at least one cave-mouth column");
        };

        let mut world = World::new(seed, 0);
        world.ensure_column(cp);
        world.column_gen.insert(cp, Arc::clone(&col));

        let cy = cave_top.div_euclid(SECTION_SIZE as i32);
        let sp = SectionPos::new(cp.cx, cy, cp.cz);
        let section = generator.generate_section(sp, &col);
        world.sections.insert(sp, Arc::new(section));

        world.recompute_column_heightmaps(cp);

        assert_eq!(
            world.columns.get(&cp).unwrap().surface_y(x, z),
            cave_top,
            "heightmap refresh must not restore original pre-cave surface {original}"
        );
    }

    #[test]
    fn heightmap_recompute_keeps_glass_out_of_direct_sky_cover() {
        let mut world = World::new(0, 0);
        let cp = ChunkPos::new(0, 0);
        let ground = SectionPos::new(0, 0, 0);
        let roof = SectionPos::new(0, 4, 0);

        let mut ground_section = Section::new(0, 0, 0);
        ground_section.set_block(8, 0, 8, Block::Stone);
        world.sections.insert(ground, Arc::new(ground_section));
        let mut roof_section = Section::new(0, 4, 0);
        roof_section.set_block(8, 0, 8, Block::Glass);
        world.sections.insert(roof, Arc::new(roof_section));

        let column = world.ensure_column(cp);
        column.set_surface_y(8, 8, 64);
        column.set_sky_cover_y(8, 8, 64);

        assert!(world.recompute_column_heightmaps(cp).is_some());
        let column = &world.columns[&cp];
        assert_eq!(column.surface_y(8, 8), 64);
        assert_eq!(
            column.sky_cover_y(8, 8),
            0,
            "saved/streamed glass must remain clear when column maps are rebuilt"
        );
    }

    #[test]
    fn heightmap_recompute_preserves_loaded_dug_shaft_below_generated_surface() {
        let seed = 0x51EED;
        let generator = ChunkGenerator::new(seed);
        let mut found = None;

        'search: for cz in -8..=8 {
            for cx in -8..=8 {
                let col = Arc::new(generator.generate_column_gen(cx, cz));
                for z in 0..SECTION_SIZE {
                    for x in 0..SECTION_SIZE {
                        let ground = col.heightmap_surface_y(x, z);
                        let lower = ground - SECTION_SIZE as i32 - 1;
                        let wx = cx * SECTION_SIZE as i32 + x as i32;
                        let wz = cz * SECTION_SIZE as i32 + z as i32;
                        if SectionPos::from_world(wx, ground, wz).is_some()
                            && SectionPos::from_world(wx, lower, wz).is_some()
                        {
                            found = Some((ChunkPos::new(cx, cz), col, x, z, ground, lower));
                            break 'search;
                        }
                    }
                }
            }
        }

        let Some((cp, col, x, z, ground, lower)) = found else {
            panic!("test seed/search window must contain a diggable surface column");
        };

        let mut world = World::new(seed, 0);
        let column = world.ensure_column(cp);
        column.set_surface_y(x, z, ground);
        column.set_sky_cover_y(x, z, ground);
        world.column_gen.insert(cp, col);

        let ground_sp = SectionPos::from_world(
            cp.cx * SECTION_SIZE as i32 + x as i32,
            ground,
            cp.cz * SECTION_SIZE as i32 + z as i32,
        )
        .unwrap();
        world.sections.insert(
            ground_sp,
            Arc::new(Section::new(cp.cx, ground_sp.cy, cp.cz)),
        );

        let lower_sp = SectionPos::from_world(
            cp.cx * SECTION_SIZE as i32 + x as i32,
            lower,
            cp.cz * SECTION_SIZE as i32 + z as i32,
        )
        .unwrap();
        let mut lower_section = Section::new(cp.cx, lower_sp.cy, cp.cz);
        lower_section.set_block(
            x,
            lower.rem_euclid(SECTION_SIZE as i32) as usize,
            z,
            Block::Stone,
        );
        world.sections.insert(lower_sp, Arc::new(lower_section));

        world.recompute_column_heightmaps(cp);

        assert_eq!(
            world.columns.get(&cp).unwrap().surface_y(x, z),
            lower,
            "a loaded dug shaft must not be covered again by the generated fallback"
        );
    }

    #[test]
    fn removing_surface_cover_relights_loaded_sections_below_the_changed_section() {
        let dir = std::env::temp_dir().join(format!(
            "petramond-sky-cover-relight-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let opened = crate::save::open_at(dir.clone()).expect("open save");
        let mut world = World::new(0, 0);
        world.attach_save(opened.save);
        let cp = ChunkPos::new(0, 0);
        let shaft_x = 8;
        let shaft_z = 8;
        let cover_y = 64;
        let top = SectionPos::new(0, 4, 0);
        let lower = SectionPos::new(0, 2, 0);

        let column = world.ensure_column(cp);
        column.set_surface_y(shaft_x, shaft_z, cover_y);
        column.set_sky_cover_y(shaft_x, shaft_z, cover_y);

        let mut top_section = Section::new(top.cx, top.cy, top.cz);
        top_section.set_block(shaft_x, 0, shaft_z, Block::Dirt);
        top_section.set_skylight(vec![0u8; SECTION_VOLUME].into());
        top_section.set_blocklight(vec![0u8; SECTION_VOLUME].into());
        top_section.dirty = false;

        let mut lower_section = Section::new(lower.cx, lower.cy, lower.cz);
        lower_section.set_skylight(vec![0u8; SECTION_VOLUME].into());
        lower_section.set_blocklight(vec![0u8; SECTION_VOLUME].into());
        lower_section.dirty = false;

        world.sections.insert(top, Arc::new(top_section));
        world.sections.insert(lower, Arc::new(lower_section));

        assert!(
            !world.sections.get(&lower).unwrap().light_dirty,
            "fixture lower section starts with settled dark skylight"
        );
        assert!(
            !world.sections.get(&lower).unwrap().dirty,
            "fixture lower section starts with no pending mesh work"
        );

        assert!(world.set_block_world(shaft_x as i32, cover_y, shaft_z as i32, Block::Air));

        assert!(
            world.sections.get(&lower).unwrap().light_dirty,
            "removing sky cover must invalidate skylight below the edited section"
        );
        assert!(
            world.light_edited_since_persist.contains(&lower),
            "distant light invalidation must be tracked in case eviction beats the rebake"
        );

        // The mark itself demands the rebake (`relight_demand`) — no mesh is
        // pre-queued for the distant section — and the landed bake's changed
        // cubes requeue its mesh.
        let mut landed = false;
        for _ in 0..2500 {
            world.pump_light_bakes();
            if !world.sections.get(&lower).unwrap().light_dirty {
                landed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(landed, "the marked distant section must rebake unprompted");
        let lower_section = world.sections.get(&lower).unwrap();
        assert_eq!(
            lower_section.skylight_at(shaft_x, 8, shaft_z),
            crate::chunk::SKY_FULL,
            "the opened shaft must reach full skylight below"
        );
        assert!(
            lower_section.dirty,
            "changed cubes must requeue the section's mesh"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn air_edit_into_absent_full_opaque_section_materializes_generated_base() {
        let seed = 0x51EED;
        let generator = ChunkGenerator::new(seed);
        let mut world = World::new(seed, 0);
        install_column_summary(&mut world, &generator, ChunkPos::new(0, 0));

        let y = SECTION_MIN_CY * SECTION_SIZE as i32;
        let sp = SectionPos::from_world(0, y, 0).unwrap();
        assert!(
            !world.sections.contains_key(&sp),
            "the deep generated-solid section starts summary-only"
        );

        assert!(world.set_block_world(0, y, 0, Block::Air));
        assert!(
            world.sections.contains_key(&sp),
            "editing virtual solid materializes the generated section"
        );
        assert_eq!(Block::from_id(world.chunk_block(0, y, 0)), Block::Air);
        assert_ne!(
            Block::from_id(world.chunk_block(1, y, 0)),
            Block::Air,
            "materialization preserves the generated solid neighbours instead of creating an empty section"
        );
    }
}
