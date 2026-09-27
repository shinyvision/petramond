//! The two halves of the client/server split, as TYPES.
//!
//! [`World<S>`](super::World) is generic over its side: the shared
//! deterministic core (voxel data, light bakes, retained draws) plus the
//! state only one side owns.
//!
//! - [`ServerSide`]: worldgen/disk streaming jobs, the replication log, the
//!   session roster and rules, the save handle, and the entity stores (mobs,
//!   dropped items, riding, schematics) the simulation drives.
//! - [`ReplicaSide`]: the client's terrain presentation — CPU meshes, the bake
//!   pools, visibility parking, prediction bundles.
//!
//! Operations that exist on one side only are inherent methods of
//! [`ServerWorld`](super::ServerWorld) or [`ReplicaWorld`](super::ReplicaWorld),
//! so calling a replica install on the server (or polling generation on a
//! replica) does not compile. Shared code that has a side-specific effect —
//! an edit queues a remesh on the replica and a replication delta on the
//! server — asks [`WorldSide`] for the side's state and gets `None` on the
//! side that has none. No runtime role flag exists.

use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::mob::Mobs;
use crate::save::WorldSave;
use crate::worker::{GenJobHandle, JobCancel, JobPool, WorkerPool};
use petramond_math::math::IVec3;
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_worldgen::cache::{CacheBudget, GenCaches};
use petramond_worldgen::ColumnGen;

use super::entities::DroppedItems;
use super::mesh_queue::DirtyMeshQueue;
use super::replication::BlockDelta;
use super::stream::StreamEvent;

mod sealed {
    /// Only this module's two sides exist; nothing outside can add a third.
    pub trait Sealed {}
}

/// Which side of the split a [`World`](super::World) plays, fixed by its type.
/// Each accessor answers `Some` on exactly the side that owns that state.
pub trait WorldSide: sealed::Sealed + Sized + 'static {
    /// The server's authority state, `None` on a replica.
    fn server(&self) -> Option<&ServerSide> {
        None
    }

    fn server_mut(&mut self) -> Option<&mut ServerSide> {
        None
    }

    /// The replica's terrain presentation, `None` on the server.
    fn replica(&self) -> Option<&ReplicaSide> {
        None
    }

    fn replica_mut(&mut self) -> Option<&mut ReplicaSide> {
        None
    }
}

/// Everything the SERVER's world owns beyond the shared core.
pub struct ServerSide {
    /// Worldgen + disk streaming job tables.
    pub(in crate::world) gen: WorldgenJobs,
    /// Per-tick change log shipped to connected sessions.
    pub(in crate::world) replication: ReplicationLog,
    /// Session roster and per-world rules.
    pub(in crate::world) session: SessionState,
    /// The worldgen worker pool the streamer submits column/section jobs to.
    pub(in crate::world) worker: WorkerPool,
    /// On-disk save handle (`None` if saving is disabled / failed to open).
    pub(in crate::world) save: Option<WorldSave>,
    /// The simulated entities and the per-tick navigation budgets they spend.
    pub(in crate::world) entities: EntityStores,
    /// The world's schematic assets and anchored ghosts (see
    /// `world::schematic`), reached through `SimCtx` by the host calls that
    /// read designs and set ghosts.
    pub(in crate::world) schematics: super::schematic::WorldSchematics,
    /// Section installs the streamer buffered for the tick-side event bus
    /// (`section_generated` / `section_loaded`); drained by the next game tick.
    pub(in crate::world) stream_events: Vec<StreamEvent>,
    /// Buffer gate, mirroring event-bus listener presence (set once per
    /// tick), so streaming costs nothing while nothing listens.
    pub(in crate::world) stream_events_enabled: bool,
}

impl ServerSide {
    pub(in crate::world) fn new(seed: u32, render_dist: i32, jobs: Arc<JobPool>) -> Self {
        Self {
            gen: WorldgenJobs::new(render_dist),
            replication: ReplicationLog::default(),
            session: SessionState::default(),
            worker: WorkerPool::new(jobs),
            save: None,
            entities: EntityStores::new(seed),
            schematics: Default::default(),
            stream_events: Vec::new(),
            stream_events_enabled: false,
        }
    }
}

impl sealed::Sealed for ServerSide {}

impl WorldSide for ServerSide {
    fn server(&self) -> Option<&ServerSide> {
        Some(self)
    }

    fn server_mut(&mut self) -> Option<&mut ServerSide> {
        Some(self)
    }
}

/// Everything a client REPLICA owns beyond the shared core: its terrain
/// presentation. A replica never generates, simulates, or persists — its
/// sections arrive as payloads (see `world::remote`).
pub struct ReplicaSide {
    pub(in crate::world) terrain: TerrainRenderState,
    /// The presented world's revisions: which keys changed when.
    pub(in crate::world) changes: super::remote::Changes,
    /// A presented replica's piece origins and piece cache.
    pub(in crate::world) provenance: Option<Box<super::remote::Provenance>>,
}

impl ReplicaSide {
    pub(in crate::world) fn new(jobs: Arc<JobPool>) -> Self {
        Self {
            terrain: TerrainRenderState::new(jobs),
            changes: super::remote::Changes::new(),
            provenance: None,
        }
    }
}

impl sealed::Sealed for ReplicaSide {}

impl WorldSide for ReplicaSide {
    fn replica(&self) -> Option<&ReplicaSide> {
        Some(self)
    }

    fn replica_mut(&mut self) -> Option<&mut ReplicaSide> {
        Some(self)
    }
}

/// The replica's terrain presentation: the CPU section meshes, the packed
/// column bookkeeping the GPU upload reads, the bake job pool, and the
/// visibility parking sets.
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
    pub(in crate::world) mesh_settle: FxHashMap<SectionPos, super::mesh_queue::settle::MeshSettle>,
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

impl TerrainRenderState {
    fn new(jobs: Arc<JobPool>) -> Self {
        Self {
            meshes: FxHashMap::default(),
            mesh_columns: FxHashSet::default(),
            mesh_column_cys: FxHashMap::default(),
            mesh_upload_revisions: FxHashMap::default(),
            mesh_upload_dirty_columns: FxHashSet::default(),
            upload_urgent_columns: FxHashSet::default(),
            mesh_release_after: FxHashMap::default(),
            repack_forced: FxHashSet::default(),
            mesh_pump_frame: 0,
            mesh_settle: FxHashMap::default(),
            mesh_pool: super::mesh_pool::MeshPool::new(jobs.clone()),
            mesh_jobs_in_flight: 0,
            mesh_job_cancels: FxHashMap::default(),
            dirty_meshes: DirtyMeshQueue::default(),
            deep_sections: FxHashSet::default(),
            visible_deep: FxHashSet::default(),
            hidden_parked: FxHashSet::default(),
            sealed_parked: FxHashSet::default(),
            prediction_terrain: super::prediction_render::PredictionTerrainQueue::new(jobs),
            vis_dirty: false,
            light_blocked_meshes: FxHashSet::default(),
        }
    }

    /// Forget every presentation record of section `pos` (it left the world):
    /// in-flight work is cancelled and the queues and parking sets drop it.
    /// The mesh itself and its column index go through `World::remove_mesh`.
    pub(in crate::world) fn forget_section(&mut self, pos: SectionPos) {
        self.prediction_terrain.cancel_section(pos);
        if let Some(job) = self.mesh_job_cancels.remove(&pos) {
            job.cancel();
        }
        self.repack_forced.remove(&pos);
        self.dirty_meshes.remove(pos);
        self.mesh_settle.remove(&pos);
        self.light_blocked_meshes.remove(&pos);
        self.deep_sections.remove(&pos);
        self.visible_deep.remove(&pos);
        self.hidden_parked.remove(&pos);
        self.sealed_parked.remove(&pos);
    }

    /// Forget every presentation record at once — the regen path.
    pub(in crate::world) fn clear(&mut self) {
        self.prediction_terrain.cancel_all();
        self.meshes.clear();
        for job in self.mesh_job_cancels.values() {
            job.cancel();
        }
        self.mesh_job_cancels.clear();
        self.mesh_settle.clear();
        self.mesh_columns.clear();
        self.mesh_column_cys.clear();
        self.mesh_upload_revisions.clear();
        self.mesh_upload_dirty_columns.clear();
        self.mesh_release_after.clear();
        self.repack_forced.clear();
        self.light_blocked_meshes.clear();
        self.deep_sections.clear();
        self.visible_deep.clear();
        self.hidden_parked.clear();
        self.sealed_parked.clear();
    }
}

/// The SERVER's worldgen + disk streaming work: which columns/sections are
/// generating or awaited, the overlay handshake, and the column records
/// queued for persistence.
pub(in crate::world) struct WorldgenJobs {
    /// Columns whose shared 2D gen data (`ColumnGen`) has landed: the source for
    /// submitting per-section jobs and sizing each column's vertical load window.
    /// Present for every loaded column; dropped when the column unloads.
    pub(in crate::world) column_gen: FxHashMap<ChunkPos, Arc<ColumnGen>>,
    /// Column generation or cache reads in flight. `None` belongs to the save
    /// thread, so the generation queue cannot reclaim that admission slot.
    pub(in crate::world) pending: FxHashMap<ChunkPos, Option<GenJobHandle>>,
    /// Sections with an in-flight per-section gen job, so the streamer never submits a
    /// section twice while it is being generated.
    pub(in crate::world) pending_sections: FxHashSet<SectionPos>,
    /// Count of `pending_sections` per XZ column. Lets settled-column slimming
    /// ask "anything still pending in this column?" in O(1) instead of rebuilding
    /// a column set from every pending section each ingest pump.
    pub(in crate::world) pending_section_columns: FxHashMap<ChunkPos, u16>,
    /// Cancellation handles for pending worker-generated sections. Disk-primary
    /// requests are in `pending_sections` without an entry here.
    pub(in crate::world) pending_section_jobs: FxHashMap<SectionPos, GenJobHandle>,
    /// A wanted section could not be admitted under the in-flight limit. The
    /// next poll retries the globally nearest missing sections after draining.
    pub(in crate::world) section_requests_unsettled: bool,
    /// Section admissions left in this load/poll phase. A burst of landed
    /// columns cannot monopolize the shared generation queue in one pump.
    pub(in crate::world) section_submit_budget: usize,
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
    /// This world's worldgen memos, sized to its view distance and worker
    /// count, and installed for every generator it builds; billed in the
    /// memory census and emptied when the world drops.
    pub(in crate::world) caches: Arc<GenCaches>,
}

impl WorldgenJobs {
    fn new(render_dist: i32) -> Self {
        let budget = CacheBudget::for_world(render_dist, JobPool::default_threads());
        let caches = Arc::new(GenCaches::new(budget));
        petramond_worldgen::cache::install(Arc::clone(&caches));
        Self {
            column_gen: FxHashMap::default(),
            pending: FxHashMap::default(),
            pending_sections: FxHashSet::default(),
            pending_section_columns: FxHashMap::default(),
            pending_section_jobs: FxHashMap::default(),
            section_requests_unsettled: false,
            section_submit_budget: super::stream::MAX_SECTION_GEN_SUBMITS_PER_PHASE,
            pending_overlays: FxHashMap::default(),
            awaited_overlays: FxHashSet::default(),
            disk_primary_sections: FxHashSet::default(),
            pending_colgen_records: Vec::new(),
            populated_columns: BTreeSet::new(),
            caches,
        }
    }
}

impl Drop for WorldgenJobs {
    fn drop(&mut self) {
        self.caches.clear();
    }
}

/// The SERVER's per-tick change log: what to ship to each session next
/// batch. Captured only while `replication_capture` is on.
#[derive(Default)]
pub(in crate::world) struct ReplicationLog {
    /// Replication log gate; ~zero cost while off (one branch at the
    /// block-change choke point). See `World::set_replication_capture`.
    pub(in crate::world) replication_capture: bool,
    /// This tick's coalesced block/water changes, latest state per cell —
    /// drained by `World::take_block_deltas`.
    pub(in crate::world) block_delta_log: FxHashMap<IVec3, BlockDelta>,
    /// This tick's coalesced per-cell KV changes, latest value per
    /// `(pos, key)` — drained by `World::take_cell_kv_deltas`. Captured
    /// behind the same [`replication_capture`](Self::replication_capture) gate.
    pub(in crate::world) cell_kv_delta_log: FxHashMap<(IVec3, String), Option<Vec<u8>>>,
    /// Cells whose DRAW SET changed this tick — drained by
    /// `World::take_block_draw_deltas` as whole sets (they are a handful of
    /// prims, and half a set draws nothing sensible).
    pub(in crate::world) block_draw_log: FxHashSet<IVec3>,
    /// Sections whose bake LANDED since the last streaming pump — drained by
    /// `World::take_light_ship_log` into per-connection `LightData` messages
    /// (filtered to each recipient's sent set). A set, so several bakes in
    /// one window ship latest-wins.
    pub(in crate::world) light_ship_log: FxHashSet<SectionPos>,
    /// Monotonic revision of "which sections exist / are stream-final": bumped
    /// on ingest, eviction, materialization, and in-flight-set changes. The
    /// per-connection terrain sender keys its wanted-vs-sent rescan on this
    /// (plus the anchor's quantized target), so a steady frame does no scan.
    pub(in crate::world) terrain_revision: u64,
}

impl ReplicationLog {
    #[inline]
    pub(in crate::world) fn bump_terrain_revision(&mut self) {
        self.terrain_revision = self.terrain_revision.wrapping_add(1);
    }

    /// Whether a per-cell KV change at `pos` needs its own delta: capture is
    /// on and no block delta covers the cell this tick (that delta's drain
    /// re-reads the cell's whole KV map, and a KV delta logged BEFORE a
    /// wiping block write would be stale — `record_block_delta` scrubs those).
    #[inline]
    pub(in crate::world) fn wants_cell_kv_delta(&self, pos: IVec3) -> bool {
        self.replication_capture && !self.block_delta_log.contains_key(&pos)
    }
}

/// Per-world session state the SERVER owns: who is connected, their queued
/// inputs, and the world rules their actions read.
pub(in crate::world) struct SessionState {
    /// Every connected player's movement intent this tick, decomposed into
    /// its own yaw frame (see [`super::session::PlayerInputSnapshot`]) —
    /// published by the server before the tick stages so the `PlayerInput`
    /// HostCall can answer from the world. Replaced wholesale each tick.
    pub(in crate::world) player_inputs: Vec<super::session::PlayerInputSnapshot>,
    /// Every connected player's state snapshot this tick (see
    /// [`super::session::PlayerRosterSnapshot`]) — published beside the
    /// inputs; the read model behind the `Players` HostCall. Replaced
    /// wholesale each tick.
    pub(in crate::world) player_roster: Vec<super::session::PlayerRosterSnapshot>,
    /// Keep the inventory on death instead of spilling it (per-world
    /// `settings.json` rule). Session-fixed, set once at open.
    pub(in crate::world) keep_inventory: bool,
    /// The full day+night cycle length in ticks (per-world `settings.json`
    /// "day length"). Session-fixed, set once at open BEFORE core systems
    /// install — the day/night cycle captures it.
    pub(in crate::world) day_cycle_ticks: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            player_inputs: Vec::new(),
            player_roster: Vec::new(),
            keep_inventory: false,
            day_cycle_ticks: super::session::DEFAULT_DAY_CYCLE_TICKS,
        }
    }
}

/// The entities the server simulates in loaded sections, plus the per-tick
/// navigation budgets their AI (and the mod ABI's `MobCanReach`) spends.
pub(in crate::world) struct EntityStores {
    /// Active dropped item entities resting in currently-loaded sections.
    pub(in crate::world) dropped_items: DroppedItems,
    /// Active mobs in currently-loaded sections.
    pub(in crate::world) mobs: Mobs,
    /// Player-on-mob riding attachments (see `mob::riding`). Reached by the
    /// mount HostCalls through `SimCtx`; the server's riding pass reconciles
    /// sessions against it each tick. Never persisted.
    pub(in crate::world) riding: crate::mob::riding::Riding,
    /// This tick's shared navigation reachability-probe budget (see
    /// `mob::nav::REACH_PROBE_TICK_BUDGET`), refilled by `tick_mobs`. It lives
    /// with the world because both askers — the mob brains and the mod ABI's
    /// `MobCanReach` — hold only the world when they ask.
    pub(in crate::world) nav_probe_budget: crate::mob::ReachBudget,
    /// This tick's budget for positional route probes (`mob::route_probe`).
    pub(in crate::world) route_probe_budget: crate::mob::ReachBudget,
    /// What reachability searches keep about the ground they search.
    pub(in crate::world) kept_boxes: crate::mob::KeptBoxes,
}

impl EntityStores {
    fn new(seed: u32) -> Self {
        Self {
            dropped_items: DroppedItems::default(),
            mobs: Mobs::new(seed as u64),
            riding: Default::default(),
            nav_probe_budget: crate::mob::ReachBudget::default(),
            route_probe_budget: crate::mob::ReachBudget::with_capacity(
                crate::mob::ROUTE_PROBE_TICK_BUDGET,
            ),
            kept_boxes: Default::default(),
        }
    }
}
