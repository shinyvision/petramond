use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{
    self, section_idx, ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::column::{Column, NO_SURFACE};
use crate::entity::DroppedItem;
use crate::mathh::{voxel_at, Vec3};
use crate::mesh::ChunkMesh;
use crate::mob::{Mobs, SavedMob};
use crate::save::{SectionSnapshot, WorldSave};
use crate::section::{Section, SectionSummary};
use crate::worker::{JobCancel, JobPool, WorkerPool};
use crate::worldgen::driver::ChunkGenerator;
use crate::worldgen::driver::ColumnGen;

use super::entities::DroppedItems;
use super::environment::WorldEnvironment;
use super::light::LightBakeQueue;
use super::mesh_queue::DirtyMeshQueue;
use super::tick::TickState;

pub const RENDER_DIST: i32 = 32;

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

/// One streaming anchor for [`World::update_load_multi`]: a player's section
/// coordinates plus that connection's streaming radius (its requested view
/// distance, already clamped by the server's own maximum), one per connected
/// player.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadAnchor {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
    /// Horizontal streaming radius in chunks for this anchor's connection.
    pub radius: i32,
}

/// Vertical load radius (in 16³ sections) around the player's section: the world
/// streams a flattened cylinder — a Euclidean horizontal disc of columns × this many
/// sections above and below the player. Sized so the visible surface band is fully
/// loaded when standing on typical terrain, while the deep underground / high sky a
/// far column doesn't need is left ungenerated until the player approaches it (the
/// per-section "generate closest to the player" payoff that makes room for caves).
pub const VERTICAL_LOAD_RADIUS: i32 = 5;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct LoadTarget {
    pub center: ChunkPos,
    /// Player's section `cy` — the centre of the vertical load window.
    pub center_cy: i32,
    pub render_dist: i32,
}

impl LoadTarget {
    pub fn new(cx: i32, cy: i32, cz: i32, render_dist: i32) -> Self {
        Self {
            center: ChunkPos::new(cx, cz),
            center_cy: cy,
            render_dist,
        }
    }

    pub(super) fn column_priority_key(self, pos: ChunkPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dz = pos.cz - self.center.cz;
        (dx as i64 * dx as i64) + (dz as i64 * dz as i64)
    }

    pub(super) fn section_priority_key(self, pos: SectionPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dy = pos.cy - self.center_cy;
        let dz = pos.cz - self.center.cz;
        (dx as i64 * dx as i64) + (dy as i64 * dy as i64) + (dz as i64 * dz as i64)
    }

    /// [`section_priority_key`](Self::section_priority_key) with a surface-first
    /// bias: while the anchor itself is above ground, a section wholly below its
    /// own column's surface band (`pos.cy < band_lo` — the same test deep
    /// classification uses) is scheduled as if it were `render_dist / 2` sections
    /// farther away. The player can only see such sections through cave openings,
    /// so the visible surface shell streams, lights, and ships first; nearby cave
    /// interiors still beat far surface rather than starving. An underground
    /// anchor (a caving player) keeps the pure 3D nearest-first order — the deep
    /// sections around them ARE the visible world.
    pub(super) fn surface_biased_section_key(
        self,
        pos: SectionPos,
        band_lo: i32,
        anchor_underground: bool,
    ) -> i64 {
        let key = self.section_priority_key(pos);
        if anchor_underground || pos.cy >= band_lo {
            return key;
        }
        let h = i64::from((self.render_dist / 2).max(8));
        key + h * h
    }
}

/// Vertical envelope of one column's direct-sky-cover changes. Skylight can
/// only differ between the lower endpoint's seep reach and the upper endpoint,
/// so streaming invalidation need not touch the rest of the world stack.
#[derive(Copy, Clone, Debug)]
pub(super) struct SkyCoverChange {
    min_cover: i32,
    max_cover: i32,
}

impl SkyCoverChange {
    pub(super) fn between(old: i32, new: i32) -> Option<Self> {
        (old != new).then_some(Self {
            min_cover: old.min(new),
            max_cover: old.max(new),
        })
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.min_cover = self.min_cover.min(other.min_cover);
        self.max_cover = self.max_cover.max(other.max_cover);
    }

    pub(super) fn affects(self, pos: SectionPos) -> bool {
        super::light::cover_change_affects_section(pos, self.min_cover, self.max_cover)
    }

    /// L1 gap from `pos`'s cell box to the changed direct-sky segment of the
    /// world column `(wx, wz)` — the cells between the two cover endpoints,
    /// whose direct-sky status flipped. Light can only change within the
    /// flood reach of that segment, so a single-column cover move needs no
    /// blanket 3×3-column invalidation.
    pub(super) fn segment_gap(self, pos: SectionPos, wx: i32, wz: i32) -> i32 {
        let (ox, oy, oz) = pos.origin_world();
        let side = SECTION_SIZE as i32 - 1;
        let gx = (ox - wx).max(wx - (ox + side)).max(0);
        let gz = (oz - wz).max(wz - (oz + side)).max(0);
        let seg_lo = self.min_cover.saturating_add(1);
        let seg_hi = self.max_cover;
        let gy = (oy - seg_hi).max(seg_lo - (oy + side)).max(0);
        gx + gz + gy
    }

    /// Generated-section ingest already invalidates that section's 3x3x3. Only
    /// an unusual cover jump spanning farther vertically needs the additional
    /// column-map invalidation pass.
    pub(super) fn escapes_section_neighborhood(self, changed: SectionPos) -> bool {
        (SECTION_MIN_CY..=SECTION_MAX_CY).any(|cy| {
            (cy - changed.cy).abs() > 1 && self.affects(SectionPos::new(changed.cx, cy, changed.cz))
        })
    }
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

    /// Install the world's "Optimize explored terrain" setting — once, at
    /// session open (like [`set_disabled_mods`](Self::set_disabled_mods)).
    pub fn set_optimize_explored_terrain(&mut self, on: bool) {
        self.optimize_explored_terrain = on;
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

    /// Attach an on-disk save: enables section persistence (load-from-disk in the
    /// streamer and flush-on-evict) and gives `Game` a handle for level/entities.
    /// Never on a replica — the server owns persistence; a replica persisting its
    /// installed copies would shadow the authoritative world.
    pub fn attach_save(&mut self, save: WorldSave) {
        debug_assert!(
            self.role != WorldRole::ClientReplica,
            "a replica must not persist replicated sections"
        );
        self.save = Some(save);
    }

    /// Turn the server-side replication log on/off (Phase C flips it on per
    /// tick while clients are connected). Turning capture off drops anything
    /// already logged, mirroring [`set_stream_event_capture`].
    ///
    /// [`set_stream_event_capture`]: Self::set_stream_event_capture
    #[allow(dead_code)] // consumed by the internal server loop (Phase C)
    pub(crate) fn set_replication_capture(&mut self, on: bool) {
        if !on {
            self.block_delta_log.clear();
        }
        self.replication_capture = on;
    }

    /// Drain this tick's coalesced block/water deltas (latest state per cell),
    /// sorted by cell so the wire batch is deterministic. Each delta's
    /// per-cell STATE is re-read here, at the drain: several placement funnels
    /// write their state maps AFTER the block write that announced the change
    /// (chest/furnace/torch insert their facing after `set_block_world`), so
    /// only the drain sees the whole tick's final state for the cell.
    pub(crate) fn take_block_deltas(&mut self) -> Vec<crate::net::protocol::BlockDelta> {
        let mut out: Vec<_> = self.block_delta_log.drain().map(|(_, d)| d).collect();
        out.sort_unstable_by_key(|d| (d.pos.x, d.pos.y, d.pos.z));
        for d in &mut out {
            // A section evicted since the write keeps the recorded state; the
            // recipient unloads it anyway.
            if self.section_loaded_at(d.pos.x, d.pos.y, d.pos.z) {
                d.state = self.cell_state_at(d.pos.x, d.pos.y, d.pos.z);
            }
        }
        out
    }

    /// Snapshot one cell's CURRENT content as a wire delta — the same shape
    /// [`record_block_delta`](Self::record_block_delta) logs, but on demand:
    /// the per-recipient corrective sync a use click that disagreed with the
    /// client's replica ships. `None` when the section is not loaded.
    pub(crate) fn block_delta_at(
        &self,
        pos: crate::mathh::IVec3,
    ) -> Option<crate::net::protocol::BlockDelta> {
        if !self.section_loaded_at(pos.x, pos.y, pos.z) {
            return None;
        }
        let block_id = self.chunk_block(pos.x, pos.y, pos.z);
        let water =
            (block_id == Block::Water.id()).then(|| self.water_meta_world(pos.x, pos.y, pos.z));
        Some(crate::net::protocol::BlockDelta {
            pos,
            block_id,
            water,
            state: self.cell_state_at(pos.x, pos.y, pos.z),
        })
    }

    /// Log the CURRENT content of one just-changed cell (called from the
    /// block-change announce choke point, after the write landed). `block_id`
    /// is the raw session id; `water` carries the meta byte iff the cell holds
    /// water. Latest write per cell per tick wins by construction; the sparse
    /// per-cell state is re-read once more at the drain (`take_block_deltas`).
    pub(super) fn record_block_delta(&mut self, wx: i32, wy: i32, wz: i32) {
        let block_id = self.chunk_block(wx, wy, wz);
        let water = (block_id == Block::Water.id()).then(|| self.water_meta_world(wx, wy, wz));
        let pos = crate::mathh::IVec3::new(wx, wy, wz);
        let state = self.cell_state_at(wx, wy, wz);
        self.block_delta_log.insert(
            pos,
            crate::net::protocol::BlockDelta {
                pos,
                block_id,
                water,
                state,
            },
        );
    }

    /// The cell's sparse per-cell block state as its wire [`CellState`], using
    /// the save codec's per-entry encodings — the delta-sized twin of the maps
    /// `Section::to_payload` ships whole. A cell carries at most one of these
    /// (`clear_on_block_change` wipes them all on any block write); a model
    /// cell folds its placed facing in.
    ///
    /// [`CellState`]: crate::net::protocol::CellState
    pub(super) fn cell_state_at(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<crate::net::protocol::CellState> {
        use crate::net::protocol::CellState;
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let s = self.sections.get(&pos)?;
        let cell = section_idx(lx, ly, lz) as u16;
        // A model cell may carry offset, facing, or both (the BASE cell of an
        // oriented multi-block records only its facing — offset [0,0,0] is
        // implicit); either one makes it a ModelCell on the wire.
        let model_off = s.model_cells().get(&cell).copied();
        let model_facing = s.model_facings().get(&cell).copied();
        if model_off.is_some() || model_facing.is_some() {
            return Some(CellState::ModelCell {
                off: model_off.unwrap_or([0, 0, 0]),
                facing: model_facing.unwrap_or_default().to_u8(),
            });
        }
        if let Some(d) = s.doors().get(&cell) {
            return Some(CellState::Door(d.encode()));
        }
        if let Some(st) = s.stair_states().get(&cell) {
            return Some(CellState::Stair(st.encode()));
        }
        if let Some(sl) = s.slab_states().get(&cell) {
            return Some(CellState::Slab([
                sl.encode_meta(),
                sl.layers[0].0,
                sl.layers[1].0,
            ]));
        }
        if let Some(a) = s.log_axes().get(&cell) {
            return Some(CellState::LogAxis(a.to_u8()));
        }
        if let Some(t) = s.torches().get(&cell) {
            return Some(CellState::Torch(t.to_u8()));
        }
        if let Some(f) = s.entity_facings().get(&cell) {
            // A furnace folds its lit state into the facing byte's high bit:
            // the replica's mesher flips the front texture from it, and a
            // lit-state delta otherwise carries nothing but this entry.
            let lit = s.furnaces().get(&cell).is_some_and(|f| f.is_lit());
            return Some(CellState::Facing(f.to_u8() | if lit { 0x80 } else { 0 }));
        }
        None
    }

    pub fn save(&self) -> Option<&WorldSave> {
        self.save.as_ref()
    }

    pub fn save_mut(&mut self) -> Option<&mut WorldSave> {
        self.save.as_mut()
    }

    /// Whether an authoritative record exists, or an explored cache record is
    /// active under this world's optimization setting.
    pub(super) fn saved_section_contains(&self, pos: SectionPos) -> bool {
        self.save.as_ref().is_some_and(|save| {
            save.authoritative_manifest_contains(pos)
                || (self.optimize_explored_terrain && save.explored_manifest_contains(pos))
        })
    }

    /// The single snapshot-and-persist gate shared by [`flush_modified_chunks`]
    /// (autosave/quit) and `unload_far_columns` (eviction). Applies the three-way
    /// persist condition and, when it holds, builds the section's [`SectionSnapshot`]
    /// with `entities`/`mobs` attached; returns `None` when the section needn't persist.
    ///
    /// The gate persists a section when ANY of:
    /// - its blocks were modified,
    /// - it carries item entities or mobs right now, or
    /// - `record_holds_entities` — its on-disk record still holds drops/mobs it no
    ///   longer carries, so the stale record must be rewritten or it resurrects them on
    ///   reload (cross-session: the caller derives it from the save handle).
    ///
    /// The caller owns the harvest policy (which fed `entities` / `mobs`) and the
    /// post-action (clear `modified` vs. evict), keeping flush's "stay active" and
    /// unload's "pause / save" lifetimes distinct.
    pub(super) fn snapshot_section_for_save(
        &self,
        pos: SectionPos,
        entities: Vec<DroppedItem>,
        mobs: Vec<SavedMob>,
        record_holds_entities: bool,
    ) -> Option<SectionSnapshot> {
        let section = self.sections.get(&pos)?;
        // Derived explored terrain and authoritative edits/entities live in
        // separate stores. First cache persistence waits for final light so the
        // common path writes and compresses the record only once.
        let light_final = !section.light_dirty || section.all_opaque();
        let authoritative_exists = self
            .save
            .as_ref()
            .is_some_and(|s| s.authoritative_manifest_contains(pos));
        let explored_exists = self
            .save
            .as_ref()
            .is_some_and(|s| s.explored_manifest_contains(pos));
        let explored_first_persist = self.optimize_explored_terrain
            && light_final
            && !authoritative_exists
            && !explored_exists;
        // A record already on disk whose light rebaked since it was written
        // (a lightless neighbour landed at the explored boundary, or an edit's
        // spill) rewrites, or its saved cubes diverge from its neighbours'.
        let relit_persisted = light_final
            && self.relit_since_persist.contains(&pos)
            && (authoritative_exists || explored_exists);
        // An edit dirtied this record's baked light and the rebake hasn't
        // landed (eviction/quit racing the bake): rewrite the record NOW —
        // the snapshot omits dirty light, so reload rebakes instead of
        // loading the pre-edit cubes as clean (a permanent dark seam).
        let light_stale_persisted = !light_final
            && self.light_edited_since_persist.contains(&pos)
            && (authoritative_exists || explored_exists);
        let authoritative =
            section.modified || !entities.is_empty() || !mobs.is_empty() || record_holds_entities;
        if authoritative || explored_first_persist || relit_persisted || light_stale_persisted {
            let mut snap = SectionSnapshot::from_section(section);
            snap.entities = entities;
            snap.mobs = mobs;
            snap.cache_only = !authoritative && !authoritative_exists;
            Some(snap)
        } else {
            None
        }
    }

    /// Snapshot every modified section to the save thread and clear the flags. Also
    /// snapshots any section holding item entities or mobs (even if its blocks are
    /// untouched) so their lifetime timers persist; they stay active in memory. Called
    /// on autosave and on quit; a no-op without an attached save.
    pub fn flush_modified_chunks(&mut self) {
        if self.save.is_none() {
            return;
        }
        // Flush's harvest policy: CLONE the resting drops and mobs (they stay active in
        // memory) so a crash can't lose them.
        let mut by_section = self.dropped_items.items_by_section();
        let mut mobs_by_section = self.mobs.saved_by_section();
        let positions: Vec<SectionPos> = self.sections.keys().copied().collect();
        let mut snaps = Vec::new();
        let mut persisted = Vec::new();
        for pos in positions {
            let entities = by_section.remove(&pos).unwrap_or_default();
            let mobs = mobs_by_section.remove(&pos).unwrap_or_default();
            let record_holds_entities = self
                .save
                .as_ref()
                .is_some_and(|s| s.record_holds_entities(pos));
            if let Some(snap) =
                self.snapshot_section_for_save(pos, entities, mobs, record_holds_entities)
            {
                snaps.push(snap);
                persisted.push(pos);
            }
        }
        // Post-action: a persisted section is now in sync with disk. The flush
        // visited EVERY loaded section, so relit bookkeeping resets wholesale
        // (evicted stragglers included — they can't re-persist anyway).
        for pos in persisted {
            if let Some(s) = self.section_mut(pos) {
                s.modified = false;
            }
            self.relit_since_persist.remove(&pos);
            self.light_edited_since_persist.remove(&pos);
        }
        if let Some(save) = self.save.as_mut() {
            save.save_sections(snaps);
        }
        self.flush_pending_colgen_records();
    }

    /// Send the buffered column-gen cache records to the save thread. Batched
    /// (autosave / unload / a size trigger in `poll`) so one region-file
    /// rewrite absorbs many columns.
    pub(super) fn flush_pending_colgen_records(&mut self) {
        if self.pending_colgen_records.is_empty() {
            return;
        }
        let recs = std::mem::take(&mut self.pending_colgen_records);
        if let Some(save) = self.save.as_mut() {
            save.save_column_gens(recs);
        }
    }

    /// Recompute a column's visible surface and direct-sky cover from its
    /// currently-loaded sections. Used after overlaying saved terrain, whose
    /// blocks can differ from generation. Returns the changed cover envelope.
    pub(super) fn recompute_column_heightmaps(&mut self, cpos: ChunkPos) -> Option<SkyCoverChange> {
        // Gather both maps under immutable section borrows, then write the
        // column once (the section and column maps are distinct fields).
        let mut surf = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut surface_remaining = surf.len();
        let mut sky_remaining = sky.len();
        for cy in Self::column_section_range().rev() {
            if surface_remaining == 0 && sky_remaining == 0 {
                break;
            }
            let Some(section) = self.sections.get(&SectionPos::new(cpos.cx, cy, cpos.cz)) else {
                continue;
            };
            let oy = cy * SECTION_SIZE as i32;
            let blocks = section.blocks_slice();
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let col = lz * SECTION_SIZE + lx;
                    if surf[col] != NO_SURFACE && sky[col] != NO_SURFACE {
                        continue;
                    }
                    for ly in (0..SECTION_SIZE).rev() {
                        let id = blocks[section_idx(lx, ly, lz)];
                        if surf[col] == NO_SURFACE && id != Block::Air.id() {
                            surf[col] = oy + ly as i32;
                            surface_remaining -= 1;
                        }
                        if sky[col] == NO_SURFACE && !Block::from_id(id).transmits_direct_skylight()
                        {
                            sky[col] = oy + ly as i32;
                            sky_remaining -= 1;
                        }
                        if surf[col] != NO_SURFACE && sky[col] != NO_SURFACE {
                            break;
                        }
                    }
                }
            }
        }
        // Floor the scan at the generated surface only while that surface section is
        // absent. Once loaded, its blocks are authoritative; otherwise a streaming
        // recompute can "restore" ground over a player-dug sky shaft.
        let bare = self.column_gen.get(&cpos).cloned();
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                let ground = bare
                    .as_ref()
                    .map(|c| c.heightmap_surface_y(lx, lz))
                    .unwrap_or(NO_SURFACE);
                let ground_loaded = SectionPos::from_world(
                    cpos.cx * SECTION_SIZE as i32 + lx as i32,
                    ground,
                    cpos.cz * SECTION_SIZE as i32 + lz as i32,
                )
                .is_some_and(|sp| self.sections.contains_key(&sp));
                if !ground_loaded && ground != NO_SURFACE {
                    surf[i] = surf[i].max(ground);
                    sky[i] = sky[i].max(ground);
                }
            }
        }
        let col = self.ensure_column(cpos);
        let mut payload_changed = false;
        let mut sky_change: Option<SkyCoverChange> = None;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                if col.surface_y(lx, lz) != surf[i] {
                    col.set_surface_y(lx, lz, surf[i]);
                    payload_changed = true;
                }
                if col.sky_cover_y(lx, lz) != sky[i] {
                    let change = SkyCoverChange::between(col.sky_cover_y(lx, lz), sky[i])
                        .expect("different cover heights");
                    if let Some(all) = sky_change.as_mut() {
                        all.merge(change);
                    } else {
                        sky_change = Some(change);
                    }
                    col.set_sky_cover_y(lx, lz, sky[i]);
                    payload_changed = true;
                }
            }
        }
        if payload_changed {
            self.bump_column_payload_revision(cpos);
        }
        sky_change
    }

    /// Merge one deterministic generated/cache section into the analytical bare
    /// surface and sky-cover maps. It can only add feature blocks above those
    /// baselines; authoritative saved terrain uses
    /// [`recompute_column_heightmaps`](Self::recompute_column_heightmaps) because
    /// it may also remove them. Returns the changed cover envelope.
    pub(super) fn raise_column_heightmaps_from_section(
        &mut self,
        pos: SectionPos,
    ) -> Option<SkyCoverChange> {
        let cpos = pos.chunk_pos();
        let oy = pos.cy * SECTION_SIZE as i32;
        let mut raised_surface = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut raised_sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let Some(section) = self.sections.get(&pos) else {
            return None;
        };
        let Some(column) = self.columns.get(&cpos) else {
            return None;
        };
        let blocks = section.blocks_slice();
        let mut any = false;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                let surface = column.surface_y(lx, lz);
                if oy + SECTION_SIZE as i32 - 1 > surface {
                    for ly in (0..SECTION_SIZE).rev() {
                        let wy = oy + ly as i32;
                        if wy <= surface {
                            break;
                        }
                        if blocks[section_idx(lx, ly, lz)] != Block::Air.id() {
                            raised_surface[i] = wy;
                            any = true;
                            break;
                        }
                    }
                }

                let sky_cover = column.sky_cover_y(lx, lz);
                if oy + SECTION_SIZE as i32 - 1 > sky_cover {
                    for ly in (0..SECTION_SIZE).rev() {
                        let wy = oy + ly as i32;
                        if wy <= sky_cover {
                            break;
                        }
                        let block = Block::from_id(blocks[section_idx(lx, ly, lz)]);
                        if !block.transmits_direct_skylight() {
                            raised_sky[i] = wy;
                            any = true;
                            break;
                        }
                    }
                }
            }
        }
        if !any {
            return None;
        }
        let column = self.columns.get_mut(&cpos).expect("column checked above");
        let mut payload_changed = false;
        let mut sky_change: Option<SkyCoverChange> = None;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                if raised_surface[i] > column.surface_y(lx, lz) {
                    column.set_surface_y(lx, lz, raised_surface[i]);
                    payload_changed = true;
                }
                if raised_sky[i] > column.sky_cover_y(lx, lz) {
                    let change = SkyCoverChange::between(column.sky_cover_y(lx, lz), raised_sky[i])
                        .expect("raised cover height");
                    if let Some(all) = sky_change.as_mut() {
                        all.merge(change);
                    } else {
                        sky_change = Some(change);
                    }
                    column.set_sky_cover_y(lx, lz, raised_sky[i]);
                    payload_changed = true;
                }
            }
        }
        if payload_changed {
            self.bump_column_payload_revision(cpos);
        }
        sky_change
    }

    /// Harvest a section into a save snapshot for UNLOAD: this DRAINS the section's
    /// resting drops/mobs into the record (pausing their lifetime timers until the
    /// section reloads), returning `None` when the section needn't persist. The persist
    /// gate is shared with autosave (`snapshot_section_for_save`).
    pub(super) fn harvest_section_snapshot(&mut self, sp: SectionPos) -> Option<SectionSnapshot> {
        if !self.sections.contains_key(&sp) {
            return None;
        }
        // The section's true content is still in flight (its saved record has not
        // been answered/applied yet): persisting the generated base now would
        // overwrite the player's on-disk record with pre-overlay state. Skip; the
        // record on disk stays authoritative. (Entities that wandered in are
        // dropped with the unload — losing a wanderer beats corrupting a build.)
        if self.awaited_overlays.contains(&sp) || self.pending_overlays.contains_key(&sp) {
            return None;
        }
        let entities = self.dropped_items.take_items_in_section(sp);
        let mobs = self.mobs.take_in_section(sp);
        let record_holds_entities = self
            .save
            .as_ref()
            .is_some_and(|s| s.record_holds_entities(sp));
        self.snapshot_section_for_save(sp, entities, mobs, record_holds_entities)
    }

    /// The active mobs (read-only), for `Game` to forward to the render-side scene
    /// adapter and to ray-test for crosshair targeting.
    #[inline]
    pub fn mobs(&self) -> &Mobs {
        &self.mobs
    }

    /// Mutable access to the active mobs.
    #[inline]
    pub fn mobs_mut(&mut self) -> &mut Mobs {
        &mut self.mobs
    }

    /// Spawn a mob and initialize its cached render light immediately, so a mob
    /// created after the mob tick does not render full-bright until the next tick.
    pub fn spawn_mob(&mut self, kind: crate::mob::Mob, pos: Vec3, yaw: f32) -> bool {
        let (sky, block) = self.mob_render_light_at(pos);
        self.mobs.spawn_lit(kind, pos, yaw, sky, block)
    }

    pub(crate) fn restore_mobs(&mut self, mobs: impl IntoIterator<Item = SavedMob>) {
        for mob in mobs {
            let (sky, block) = self.mob_render_light_at(mob.pos);
            self.mobs.restore_saved_mob_lit(mob, sky, block);
        }
    }

    fn mob_render_light_at(&self, pos: Vec3) -> (u8, u8) {
        let c = voxel_at(pos + Vec3::new(0.0, 0.3, 0.0));
        let sky = self.skylight6_at_world(c.x, c.y, c.z);
        let block = self.blocklight6_at_world(c.x, c.y, c.z);
        (sky, block)
    }

    /// Record one gameplay noise for the mob AI's hearing batch (see
    /// `mob::noise` for the timing contract). Emitters are the game's own
    /// funnels: player steps, block place/break.
    pub fn push_noise(&mut self, noise: crate::mob::Noise) {
        self.mobs.push_noise(noise);
    }

    /// Advance the mobs one fixed game tick against an immutable view of the rest of
    /// the world (the field is moved out so the `&mut Mobs` and `&World` borrows stay
    /// disjoint). Returns the gameplay events mobs produced this tick, for `Game` to
    /// apply through the relevant damage pipelines.
    pub fn tick_mobs(
        &mut self,
        dt: f32,
        anchors: &[crate::mob::PlayerAnchor],
    ) -> crate::mob::MobTickEvents {
        if self.mobs.is_empty() {
            // Nobody is listening: drop the tick's noise batch, or a mob-free
            // world would accumulate the player's footsteps forever.
            self.mobs.discard_noises();
            return crate::mob::MobTickEvents::default();
        }
        let freeze_unloaded = self.save.is_some();
        let mut mobs = std::mem::take(&mut self.mobs);
        let attacks = mobs.tick(dt, self, anchors, freeze_unloaded);
        self.mobs = mobs;
        attacks
    }

    /// Run one natural mob-spawn attempt (the passive backfill trickle; the
    /// caller owns the cadence). Returns the mobs actually spawned, for the
    /// caller to report as `mob_spawned` events.
    pub fn spawn_mobs_tick(&mut self, player_pos: Vec3) -> Vec<(crate::mob::Mob, Vec3)> {
        let mut mobs = std::mem::take(&mut self.mobs);
        let spawned = mobs.spawn_tick(self, player_pos);
        self.mobs = mobs;
        spawned
    }

    /// Run one worldgen-population step around `player_pos` (see `mob::populate`):
    /// place the one-time herds of nearby chunks whose deterministic roll says so,
    /// and record the chunks that spawned in the persisted populated set. Returns
    /// the mobs spawned, for the caller's `mob_spawned` events.
    pub fn populate_mobs_tick(&mut self, player_pos: Vec3) -> Vec<(crate::mob::Mob, Vec3)> {
        let mut mobs = std::mem::take(&mut self.mobs);
        let (spawned, populated) = mobs.populate_tick(self, player_pos);
        self.mobs = mobs;
        self.populated_columns.extend(populated);
        spawned
    }

    /// Whether `chunk`'s one-time worldgen herd already spawned (this session or
    /// any earlier one — the set is restored from `level.dat` at world open).
    pub fn column_populated(&self, chunk: ChunkPos) -> bool {
        self.populated_columns.contains(&chunk)
    }

    /// The persisted populated-chunk set, for the `level.dat` encoder.
    pub fn populated_columns(&self) -> &BTreeSet<ChunkPos> {
        &self.populated_columns
    }

    /// Restore the populated-chunk set at world open (before the first tick, so
    /// the first population pass already sees every historical herd).
    pub fn set_populated_columns(&mut self, set: BTreeSet<ChunkPos>) {
        self.populated_columns = set;
    }

    #[inline]
    pub fn lighting_revision(&self) -> u64 {
        self.lighting_revision
    }

    pub(super) fn bump_lighting_revision(&mut self) {
        self.lighting_revision = self.lighting_revision.wrapping_add(1);
    }

    pub(super) fn mark_light_dirty_pos(&mut self, pos: SectionPos) {
        if let Some(s) = self.section_mut(pos) {
            s.mark_light_dirty();
            // Headless rebakes are demanded from the mark itself (no mesh
            // pump to demand them). Client/combined section-grained marks
            // (ingest, unload, topology) stay mesh-demanded, so far edges
            // keep their dormant-until-visible behaviour; EDIT marks demand
            // explicitly via `mark_light_dirty_demanded`.
            if self.role == WorldRole::ServerHeadless {
                self.relight_demand.insert(pos);
            }
        }
        if self.light_deferred.contains(&pos) {
            self.deferred_rechecks.insert(pos);
        }
    }

    /// [`mark_light_dirty_pos`](Self::mark_light_dirty_pos) plus a direct
    /// bake demand: edit-driven invalidation must rebake even when no queued
    /// mesh demands the section (a distant sky-cover segment pre-marks no
    /// meshes — the landed bake's diff requeues them if anything changed).
    pub(super) fn mark_light_dirty_demanded(&mut self, pos: SectionPos) {
        self.mark_light_dirty_pos(pos);
        if self.sections.contains_key(&pos) {
            self.relight_demand.insert(pos);
        }
    }

    pub(super) fn queue_dirty_mesh(&mut self, pos: SectionPos) {
        // A headless server never meshes: with nobody pumping `tick_mesh_budget`,
        // anything queued here would only accumulate.
        if self.role == WorldRole::ServerHeadless {
            return;
        }
        if let Some(job) = self.mesh_job_cancels.get(&pos) {
            job.cancel();
        }
        if let Some(s) = self.section_mut(pos) {
            s.dirty = true;
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
            self.light_blocked_meshes.remove(&pos);
            self.hidden_parked.remove(&pos);
            self.sealed_parked.remove(&pos);
            self.dirty_meshes.push(pos);
        }
    }

    pub(super) fn mark_light_and_mesh_dirty_pos(&mut self, pos: SectionPos) {
        self.mark_light_dirty_pos(pos);
        self.queue_dirty_mesh(pos);
    }

    /// A column sky-cover cell moved, changing which cells are considered open sky
    /// for every skylight bake whose 3×3 XZ seed grid includes this column. Dirty only
    /// sections already in memory; absent generated/sky sections will bake from the new
    /// cover map when they stream in or materialize.
    /// Streaming cover changes arrive in contiguous batches, so each loaded
    /// section is invalidated only once even when several changed columns'
    /// dependent footprints overlap.
    pub(super) fn mark_sky_cover_light_dirty_around_many(
        &mut self,
        changes: impl IntoIterator<Item = (ChunkPos, SkyCoverChange)>,
    ) {
        self.mark_sky_cover_light_dirty_around_impl(changes, false);
    }

    /// Gameplay-edit variant of the sky-cover invalidation, exact to the
    /// changed world column: only sections within light-flood reach of the
    /// flipped direct-sky segment relight. No meshes are pre-marked — a
    /// landed rebake requeues its own mesh and the samplers of whatever
    /// regions actually changed (`pump_light_bakes`), so an envelope section
    /// whose cubes prove untouched publishes nothing. Also records that any
    /// persisted cubes are stale so an eviction racing the rebake rewrites
    /// them lightless.
    pub(super) fn mark_sky_cover_edited_at(&mut self, wx: i32, wz: i32, change: SkyCoverChange) {
        // Exact flood reach (14, see `LIGHT_REACH`); the streaming batch path
        // keeps its column-level `SKY_SEEP_REACH` bound.
        const LIGHT_REACH: i32 = chunk::SKY_FULL as i32 / 2 - 1;
        let center = ChunkPos::new(
            wx.div_euclid(SECTION_SIZE as i32),
            wz.div_euclid(SECTION_SIZE as i32),
        );
        let note_persist = self.save.is_some();
        for dz in -1..=1 {
            for dx in -1..=1 {
                for cy in Self::column_section_range() {
                    let pos = SectionPos::new(center.cx + dx, cy, center.cz + dz);
                    if change.segment_gap(pos, wx, wz) > LIGHT_REACH
                        || !self.sections.contains_key(&pos)
                    {
                        continue;
                    }
                    if note_persist {
                        self.light_edited_since_persist.insert(pos);
                    }
                    self.mark_light_dirty_demanded(pos);
                }
            }
        }
    }

    fn mark_sky_cover_light_dirty_around_impl(
        &mut self,
        changes: impl IntoIterator<Item = (ChunkPos, SkyCoverChange)>,
        edited: bool,
    ) {
        let mut affected = Vec::new();
        let mut seen = FxHashSet::default();
        for (center, change) in changes {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    for cy in Self::column_section_range() {
                        let pos = SectionPos::new(center.cx + dx, cy, center.cz + dz);
                        if change.affects(pos)
                            && self.sections.contains_key(&pos)
                            && seen.insert(pos)
                        {
                            affected.push(pos);
                        }
                    }
                }
            }
        }
        for pos in affected {
            if edited && self.save.is_some() {
                self.light_edited_since_persist.insert(pos);
            }
            self.mark_light_and_mesh_dirty_pos(pos);
        }
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

    /// Whether the column is loaded, and if so its payload revision — the
    /// change-detection half of [`client_surface_column`](Self::client_surface_column).
    pub(crate) fn client_surface_column_revision(&self, pos: ChunkPos) -> Option<u64> {
        self.columns
            .contains_key(&pos)
            .then(|| self.column_payload_revision(pos))
    }

    /// Final top-down surface samples for one whole chunk column, for
    /// presentation-only client modules: per cell `(height, rgb)`, or `None`
    /// where the cell is unknown (missing data or a surface section still in
    /// flight — never guessed from generation; callers retain prior explored
    /// samples). Returns `false` when the column itself is not loaded.
    ///
    /// Column, section finality, and the 5×5 biome tint blend are resolved
    /// once per column / per section / per tint kind, not per cell — this is
    /// the sampling hot path.
    pub(crate) fn client_surface_column(
        &self,
        pos: ChunkPos,
        out: &mut [Option<(i16, [u8; 3])>; 256],
    ) -> bool {
        let Some(column) = self.columns.get(&pos) else {
            return false;
        };
        let mut tints = SurfaceTintGrids::new(self, pos, column);
        // Surface heights cluster in a handful of sections per column.
        let mut sections: Vec<(i32, Option<&Section>)> = Vec::new();
        for lz in 0..16usize {
            for lx in 0..16usize {
                let i = lz * 16 + lx;
                out[i] = None;
                let height = column.surface_y(lx, lz);
                if height == NO_SURFACE {
                    continue;
                }
                let cy = height.div_euclid(SECTION_SIZE as i32);
                let section = match sections.iter().find(|(known, _)| *known == cy) {
                    Some((_, section)) => *section,
                    None => {
                        let sp = SectionPos::new(pos.cx, cy, pos.cz);
                        let section = (SectionPos::cy_in_range(cy) && self.stream_writable(sp))
                            .then(|| self.sections.get(&sp).map(Arc::as_ref))
                            .flatten();
                        sections.push((cy, section));
                        section
                    }
                };
                let Some(section) = section else {
                    continue;
                };
                let block = section.block(lx, height.rem_euclid(SECTION_SIZE as i32) as usize, lz);
                let tile = block.tiles()[0];
                let base = tile.map_rgb();
                let rgb = match tile.world_tint() {
                    None => base,
                    Some(kind) => {
                        let tint = tints.at(kind, lx, lz);
                        std::array::from_fn(|channel| {
                            (base[channel] as f32 * tint[channel])
                                .round()
                                .clamp(0.0, 255.0) as u8
                        })
                    }
                };
                out[i] = Some((height as i16, rgb));
            }
        }
        true
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

    /// Mark the 3×3×3 section neighbourhood around `center` dirty for remesh, so
    /// border face-culling / AO / light sampling stay correct across section seams.
    pub(super) fn mark_dirty_neighborhood(&mut self, center: SectionPos, include_center: bool) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if !include_center && dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    self.queue_dirty_mesh(SectionPos::new(
                        center.cx + dx,
                        center.cy + dy,
                        center.cz + dz,
                    ));
                }
            }
        }
    }

    /// L1 distance from a section-local cell to the nearest cell of the
    /// neighbouring section at axis delta `d` (0 within this section).
    #[inline]
    pub(super) fn axis_gap(local: usize, d: i32) -> i32 {
        match d {
            -1 => local as i32 + 1,
            1 => SECTION_SIZE as i32 - local as i32,
            _ => 0,
        }
    }

    /// The exact flood reach of one changed cell, in cells: the flood loses 2
    /// per step from at most `SKY_FULL` (30), so no single-cell change
    /// survives past L1 distance 14.
    pub(super) const LIGHT_REACH: i32 = chunk::SKY_FULL as i32 / 2 - 1;

    /// Mark light dirty for exactly the sections one changed cell can
    /// influence within `radius` (at most
    /// [`LIGHT_REACH`](Self::LIGHT_REACH), so a mid-section edit invalidates
    /// 7 sections, not 27; smaller when the caller bounded the reach by the
    /// light actually present at the cell — an edit in the dark cannot
    /// brighten or darken anything far away). Direct-sky cover moves reach
    /// farther vertically and are invalidated separately via
    /// [`SkyCoverChange`] segments.
    pub(super) fn mark_light_dirty_around_cell_radius(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        radius: i32,
    ) {
        let Some((center, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return;
        };
        let note_persist = self.save.is_some();
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let gap =
                        Self::axis_gap(lx, dx) + Self::axis_gap(ly, dy) + Self::axis_gap(lz, dz);
                    if gap > radius {
                        continue;
                    }
                    let pos = SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz);
                    self.mark_light_dirty_demanded(pos);
                    if note_persist && self.sections.contains_key(&pos) {
                        self.light_edited_since_persist.insert(pos);
                    }
                }
            }
        }
    }

    /// Queue a remesh of every section whose mesh samples world cell
    /// `(wx, wy, wz)`: the owning section plus the bordering neighbours whose
    /// one-cell 18³ pad (face culling, AO, smooth light) includes it — up to 8
    /// sections for a corner cell, 1 for an interior cell. Sections whose
    /// *light* the edit changes are requeued when their rebake lands (see
    /// `pump_light_bakes`), so they need no blanket pre-mark here.
    pub(super) fn queue_dirty_meshes_sampling_cell(&mut self, wx: i32, wy: i32, wz: i32) {
        #[inline]
        fn deltas(local: usize) -> &'static [i32] {
            if local == 0 {
                &[0, -1]
            } else if local == SECTION_SIZE - 1 {
                &[0, 1]
            } else {
                &[0]
            }
        }
        let Some((center, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return;
        };
        for &dy in deltas(ly) {
            for &dz in deltas(lz) {
                for &dx in deltas(lx) {
                    self.queue_dirty_mesh(SectionPos::new(
                        center.cx + dx,
                        center.cy + dy,
                        center.cz + dz,
                    ));
                }
            }
        }
    }

    pub(super) fn mark_light_dirty_neighborhood(
        &mut self,
        center: SectionPos,
        include_center: bool,
    ) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if !include_center && dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    self.mark_light_dirty_pos(SectionPos::new(
                        center.cx + dx,
                        center.cy + dy,
                        center.cz + dz,
                    ));
                }
            }
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

/// Lazy per-column 5×5 biome-blended tint grids for surface sampling: one
/// 16×16 grid per [`TileTint`] kind, built on first use. With a 20×20 biome
/// halo the blend is a separable box sum (each halo biome color decodes once);
/// without one it falls back to the column's own unblended biome colors.
struct SurfaceTintGrids<'a> {
    halo: Option<&'a [u8]>,
    column: &'a Column,
    grids: [Option<Box<[[f32; 3]; 256]>>; 3],
}

impl<'a> SurfaceTintGrids<'a> {
    fn new(world: &'a World, pos: ChunkPos, column: &'a Column) -> Self {
        let halo = world
            .column_gen
            .get(&pos)
            .map(|column| column.mesh_biome_slice())
            .or_else(|| world.column_biome_halos.get(&pos).map(|halo| halo.as_ref()))
            .filter(|halo| halo.len() == 20 * 20);
        Self {
            halo,
            column,
            grids: [None, None, None],
        }
    }

    fn at(&mut self, kind: crate::atlas::TileTint, lx: usize, lz: usize) -> [f32; 3] {
        let slot = match kind {
            crate::atlas::TileTint::Grass => &mut self.grids[0],
            crate::atlas::TileTint::Foliage => &mut self.grids[1],
            crate::atlas::TileTint::Water => &mut self.grids[2],
        };
        slot.get_or_insert_with(|| Self::build(self.halo, self.column, kind))[lz * 16 + lx]
    }

    fn build(
        halo: Option<&[u8]>,
        column: &Column,
        kind: crate::atlas::TileTint,
    ) -> Box<[[f32; 3]; 256]> {
        let color_of = |id: u8| {
            let biome = crate::biome::Biome::from_id(id);
            match kind {
                crate::atlas::TileTint::Grass => biome.grass_color(),
                crate::atlas::TileTint::Foliage => biome.foliage_color(),
                crate::atlas::TileTint::Water => biome.water_color(),
            }
        };
        let mut out = Box::new([[0.0f32; 3]; 256]);
        let Some(halo) = halo else {
            for lz in 0..16 {
                for lx in 0..16 {
                    out[lz * 16 + lx] = color_of(column.biome_at(lx, lz));
                }
            }
            return out;
        };
        let mut colors = [[0.0f32; 3]; 400];
        for (color, &id) in colors.iter_mut().zip(halo) {
            *color = color_of(id);
        }
        // The halo starts two cells before the column, so the 5x5 blend window
        // for local (x,z) occupies [x..x+5, z..z+5] directly.
        let mut rows = [[0.0f32; 3]; 20 * 16];
        for z in 0..20 {
            for x in 0..16 {
                let mut sum = [0.0f32; 3];
                for cell in &colors[z * 20 + x..z * 20 + x + 5] {
                    for channel in 0..3 {
                        sum[channel] += cell[channel];
                    }
                }
                rows[z * 16 + x] = sum;
            }
        }
        for z in 0..16 {
            for x in 0..16 {
                let mut sum = [0.0f32; 3];
                for row in 0..5 {
                    for channel in 0..3 {
                        sum[channel] += rows[(z + row) * 16 + x][channel];
                    }
                }
                out[z * 16 + x] = sum.map(|channel| channel / 25.0);
            }
        }
        out
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
