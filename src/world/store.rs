use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::worker::JobPool;
use petramond_math::math::IVec3;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::{Section, SectionSummary};
use petramond_worldgen::{ChunkGenerator, SectionGen};

use super::light::LightBakeQueue;
use super::side::{ReplicaSide, ServerSide, WorldSide};

// Moved halves, re-exported under their historical `store::` paths.
pub use petramond_world::world::column_heightmaps::SkyCoverChange;
pub use petramond_world::world::data::{ContentState, WorldData};
pub use petramond_world::world::load_targets::{
    LoadAnchor, LoadTarget, RENDER_DIST, VERTICAL_LOAD_RADIUS,
};

pub(in crate::world) mod block_entity_index;
mod evict;
mod memory;
mod mesh_index;
mod section_index;

pub use memory::MemoryCensus;
pub(in crate::world) use section_index::{column_cy_bit, for_each_column_cy};

#[cfg(any(test, feature = "test-support"))]
mod fixtures;
#[cfg(test)]
mod tests;

/// Retained DRAW state: per-block draw sets submitted for placed cells,
/// indexed by section. Authoritative on the server, replicated on a replica.
#[derive(Default)]
pub(in crate::world) struct DrawStore {
    /// Per-cell DRAW SETS: retained presentation geometry submitted for a
    /// placed block, redrawn every frame with no re-mesh. Sparse (empty in
    /// almost every world) and per-cell, exactly like `custom_bake`.
    pub(in crate::world) block_draws: FxHashMap<IVec3, crate::world::draw::PlacedDraw>,
    /// The same sets indexed by the SECTION their anchor sits in, with a union
    /// bound per section. Every consumer of this store is section-shaped (a
    /// section payload, an eviction, a frame's view cull), and each of them
    /// walked the whole map before this index existed.
    pub(in crate::world) block_draw_sections: FxHashMap<SectionPos, crate::world::draw::SectionDraws>,
}

/// The cubic voxel world: a sparse 3D grid of 16³ [`Section`]s plus a sparse 2D
/// grid of per-column data (biome, visible surface, direct-sky cover), the
/// light bakes over them, and the retained per-block draws. Sections are the
/// unit of storage, meshing, lighting, streaming, and saving.
///
/// `S` is the side of the client/server split this world plays (see
/// [`WorldSide`]): a [`ServerWorld`] generates, simulates, replicates and
/// persists; a [`ReplicaWorld`] installs what the server ships and meshes it.
/// The deterministic data half is reachable read-only through
/// [`data`](Self::data); every mutation goes through a `World` method so the
/// derived indexes, light/mesh invalidation and replication stay in step.
pub struct World<S: WorldSide> {
    /// The deterministic world half. Orchestration code that split-borrows
    /// writes `self.data.` explicitly; outside `world`, [`World::data`].
    pub(in crate::world) data: WorldData,
    /// Off-thread light bakes. Both sides light: the server for the ship gate
    /// and persistence, a replica for its own predicted edits.
    pub(in crate::world) light_bakes: LightBakeQueue,
    /// Mod-submitted per-block draw sets.
    pub(in crate::world) draws: DrawStore,
    /// The side-specific state; its type fixes which operations exist.
    pub(in crate::world) side: S,
}

/// The server's simulation world: gen + light + sim + replication capture +
/// persistence, no meshing.
pub type ServerWorld = World<ServerSide>;

/// A client's replica: sections installed from the connection, its own light
/// for predicted edits, and the terrain meshes the renderer draws.
pub type ReplicaWorld = World<ReplicaSide>;

impl<S: WorldSide> World<S> {
    fn with_side(seed: u32, render_dist: i32, jobs: &Arc<JobPool>, side: S) -> Self {
        Self {
            data: WorldData::new(seed, render_dist),
            light_bakes: LightBakeQueue::new(jobs.clone()),
            draws: DrawStore::default(),
            side,
        }
    }

    /// Read-only access to the deterministic world half: block, light, fluid,
    /// shape and collision queries.
    #[inline]
    pub fn data(&self) -> &WorldData {
        &self.data
    }

    /// Change the view/streaming radius live (the Options view-distance
    /// slider). On a server the next `update_load*` re-shapes the working set
    /// (anchor radii clamp to this budget); on a replica it re-shapes
    /// mesh/light scheduling around the view center.
    pub fn set_render_dist(&mut self, chunks: i32) {
        let chunks = chunks.max(1);
        if self.data.render_dist == chunks {
            return;
        }
        self.data.render_dist = chunks;
        self.mark_visibility_dirty();
    }

    /// Plane openness or the view changed: a replica re-runs its
    /// deep-visibility pass before the next mesh submission. The server has no
    /// visibility to refresh.
    #[inline]
    pub(in crate::world) fn mark_visibility_dirty(&mut self) {
        if let Some(replica) = self.side.replica_mut() {
            replica.terrain.vis_dirty = true;
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
        if !self.data.stream_writable(pos) {
            return false;
        }
        if !self.data.sections.contains_key(&pos) {
            if self.data.saved_section_contains(pos) {
                return false;
            }
            let col = self
                .column_gen(pos.chunk_pos())
                .filter(|col| col.section_summary(pos.cy) != SectionSummary::Empty);
            let section = match col {
                Some(col) => match ChunkGenerator::shared(self.data.seed).start_section(pos, col) {
                    SectionGen::Ready(section) => section,
                    // A mod hook waits on a fact another worker is deriving:
                    // refuse the write rather than stall the tick on it.
                    SectionGen::Deferred(_) => return false,
                },
                None => Section::new(pos.cx, pos.cy, pos.cz),
            };
            self.data.ensure_column(pos.chunk_pos());
            self.data.sections.insert(pos, Arc::new(section));
            self.note_section_loaded(pos);
            self.refresh_block_entity_index(pos);
            self.refresh_particle_emitter_index(pos);
            // A synchronously-born section must enter connected clients' sent
            // shapes promptly, or its deltas are filtered until an anchor move.
            self.bump_terrain_revision();
        }
        true
    }

    /// [`materialize_section`](Self::materialize_section) for the section owning world
    /// cell `c`. Returns `false` if `c` is outside the world vertical range.
    pub(super) fn materialize_section_at(&mut self, c: IVec3) -> bool {
        match SectionPos::from_world(c.x, c.y, c.z) {
            Some(sp) => self.materialize_section(sp),
            None => false,
        }
    }

    /// A column's landed generation data — the server's streaming facts. A
    /// replica never generates, so it answers `None` and falls back to the
    /// shipped column summaries.
    #[inline]
    pub(in crate::world) fn column_gen(
        &self,
        pos: ChunkPos,
    ) -> Option<&Arc<petramond_worldgen::ColumnGen>> {
        self.side.server()?.gen.column_gen.get(&pos)
    }

    /// Whether a save is attached: edits then track which persisted light
    /// cubes they made stale. Only a server persists.
    #[inline]
    pub(in crate::world) fn persisting(&self) -> bool {
        self.side.server().is_some_and(|s| s.save.is_some())
    }

    /// Bump the server's terrain revision (see
    /// `ReplicationLog::terrain_revision`) after a change to which sections
    /// exist or are final. A replica ships nothing, so it has no revision.
    #[inline]
    pub(super) fn bump_terrain_revision(&mut self) {
        if let Some(server) = self.side.server_mut() {
            server.replication.bump_terrain_revision();
        }
    }
}

impl ServerWorld {
    /// A test server world over an INLINE job pool (see [`JobPool::inline`]):
    /// every gen/light job it queues finishes before the submitting call
    /// returns, so a test gates on one more pump instead of sleeping on a
    /// worker thread, and a test process never spawns a pool per world.
    /// Production worlds share the session's pool through
    /// [`with_pool`](Self::with_pool).
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self::with_pool(seed, render_dist, Arc::new(JobPool::inline()))
    }

    /// Construct over a caller-owned job pool, so the server world and the
    /// local client's replica — which run in one process — can share one pool
    /// instead of each spawning a machine-sized thread set. ONE pool serves
    /// every streaming stage; the per-stage adapters each hold a handle and
    /// compete purely on distance priority.
    pub fn with_pool(seed: u32, render_dist: i32, jobs: Arc<JobPool>) -> Self {
        let side = ServerSide::new(seed, render_dist, jobs.clone());
        Self::with_side(seed, render_dist, &jobs, side)
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
        let gen = &self.side.gen;
        if !gen.pending_sections.contains(&sp)
            && !gen.awaited_overlays.contains(&sp)
            && !gen.pending_overlays.contains_key(&sp)
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
        col: Arc<petramond_worldgen::ColumnGen>,
    ) {
        let summaries: Box<[SectionSummary]> = WorldData::column_section_range()
            .map(|cy| col.section_summary(cy))
            .collect();
        self.data.column_summaries.insert(pos, summaries);
        self.side.gen.column_gen.insert(pos, col);
    }

    /// Rebuild `stream_nonfinal` wholesale from the three in-flight sets —
    /// the bulk (`clear`) counterpart of the per-section maintainers.
    pub(super) fn rebuild_stream_nonfinal(&mut self) {
        let gen = &self.side.gen;
        self.data.stream_nonfinal = gen
            .pending_sections
            .iter()
            .chain(gen.awaited_overlays.iter())
            .chain(gen.pending_overlays.keys())
            .copied()
            .collect();
    }

    /// Keep the inventory on death (per-world `settings.json` rule).
    #[inline]
    pub fn keep_inventory(&self) -> bool {
        self.side.session.keep_inventory
    }

    /// Install the keep-inventory rule — once, at session open.
    pub fn set_keep_inventory(&mut self, keep: bool) {
        self.side.session.keep_inventory = keep;
    }

    /// The full day+night cycle length in ticks (per-world "day length").
    #[inline]
    pub fn day_cycle_ticks(&self) -> u64 {
        self.side.session.day_cycle_ticks
    }

    /// Install the world's cycle length — once, at session open, BEFORE core
    /// systems install (the day/night cycle captures it).
    pub fn set_day_cycle_ticks(&mut self, ticks: u64) {
        self.side.session.day_cycle_ticks = ticks.max(1);
    }

    /// Replace the published per-player input snapshots for this tick (see
    /// [`super::session::PlayerInputSnapshot`]).
    pub fn set_player_inputs(&mut self, inputs: Vec<super::session::PlayerInputSnapshot>) {
        self.side.session.player_inputs = inputs;
    }

    /// The published input snapshot for `player`, if connected this tick.
    pub fn player_input(&self, player: u8) -> Option<super::session::PlayerInputSnapshot> {
        self.side
            .session
            .player_inputs
            .iter()
            .find(|i| i.id == player)
            .copied()
    }

    /// Replace the published per-player roster for this tick (see
    /// [`super::session::PlayerRosterSnapshot`]).
    pub fn set_player_roster(&mut self, roster: Vec<super::session::PlayerRosterSnapshot>) {
        self.side.session.player_roster = roster;
    }

    /// Every connected player's published snapshot this tick, session-id order.
    pub fn player_roster(&self) -> &[super::session::PlayerRosterSnapshot] {
        &self.side.session.player_roster
    }

    /// The worldgen memo report behind [`MemoryCensus::worldgen_cache_bytes`].
    pub fn worldgen_cache_report(&self) -> Vec<petramond_worldgen::cache::MemoStats> {
        self.side.gen.caches.report()
    }

    /// Monotonic revision of which sections exist / are stream-final (see
    /// `ReplicationLog::terrain_revision`).
    #[inline]
    pub fn terrain_revision(&self) -> u64 {
        self.side.replication.terrain_revision
    }

    /// This tick's shared navigation probe budget.
    #[inline]
    pub fn reach_budget(&self) -> &crate::mob::ReachBudget {
        &self.side.entities.nav_probe_budget
    }

    #[inline]
    pub(crate) fn kept_boxes(&self) -> &crate::mob::KeptBoxes {
        &self.side.entities.kept_boxes
    }

    /// This tick's positional route probe budget.
    #[inline]
    pub fn route_probe_budget(&self) -> &crate::mob::ReachBudget {
        &self.side.entities.route_probe_budget
    }
}

impl ReplicaWorld {
    /// A test replica over an INLINE job pool (see [`ServerWorld::new`]):
    /// mesh and light jobs finish inside the call that queues them.
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self::with_pool(seed, render_dist, Arc::new(JobPool::inline()))
    }

    /// Construct over a caller-owned job pool (see [`ServerWorld::with_pool`]).
    pub fn with_pool(seed: u32, render_dist: i32, jobs: Arc<JobPool>) -> Self {
        let side = ReplicaSide::new(jobs.clone());
        Self::with_side(seed, render_dist, &jobs, side)
    }
}

impl<S: WorldSide> petramond_world::block::behavior::BehaviorWorld for World<S> {
    fn data(&self) -> &WorldData {
        &self.data
    }

    fn queue_block_hook(&mut self, hook: petramond_world::block::behavior::BlockHook) {
        self.data.queue_block_hook(hook);
    }

    fn set_block_world(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        b: petramond_world::block::Block,
    ) -> bool {
        World::set_block_world(self, wx, wy, wz, b)
    }

    fn break_block_naturally(&mut self, pos: IVec3) {
        World::break_block_naturally(self, pos)
    }
}
