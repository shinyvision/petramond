use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::worker::JobPool;
use petramond_math::math::IVec3;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::{Section, SectionSummary};
use petramond_worldgen::{ChunkGenerator, SectionGen};

use super::light::LightBakeQueue;
use super::side::{ReplicaSide, ServerSide, WorldSide};

pub use petramond_world::world::column_heightmaps::SkyCoverChange;
pub use petramond_world::world::data::WorldData;
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

#[derive(Default)]
pub(in crate::world) struct DrawStore {
    pub(in crate::world) block_draws: FxHashMap<IVec3, crate::world::draw::PlacedDraw>,
    pub(in crate::world) block_draw_sections:
        FxHashMap<SectionPos, crate::world::draw::SectionDraws>,
}

pub struct World<S: WorldSide> {
    pub(in crate::world) data: WorldData,
    pub(in crate::world) light_bakes: LightBakeQueue,
    pub(in crate::world) draws: DrawStore,
    pub(in crate::world) side: S,
}

pub type ServerWorld = World<ServerSide>;

pub type ReplicaWorld = World<ReplicaSide>;

impl<S: WorldSide> World<S> {
    fn with_side(seed: u32, render_dist: i32, jobs: &Arc<JobPool>, side: S) -> Self {
        let mut data = WorldData::new(seed, render_dist);
        data.column_revision_counter = petramond_world::world::data::revision_base();
        Self {
            data,
            light_bakes: LightBakeQueue::new(jobs.clone()),
            draws: DrawStore::default(),
            side,
        }
    }

    #[inline]
    pub fn data(&self) -> &WorldData {
        &self.data
    }

    pub fn set_render_dist(&mut self, chunks: i32) {
        let chunks = chunks.max(1);
        if self.data.render_dist == chunks {
            return;
        }
        self.data.render_dist = chunks;
        self.mark_visibility_dirty();
    }

    #[inline]
    pub(in crate::world) fn mark_visibility_dirty(&mut self) {
        if let Some(replica) = self.side.replica_mut() {
            replica.terrain.vis_dirty = true;
        }
    }

    pub(super) fn materialize_section(&mut self, pos: SectionPos) -> bool {
        if !SectionPos::cy_in_range(pos.cy) {
            return false;
        }
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
                    SectionGen::Deferred(_) => return false,
                },
                None => Section::new(pos.cx, pos.cy, pos.cz),
            };
            self.data.ensure_column(pos.chunk_pos());
            self.data.sections.insert(pos, Arc::new(section));
            self.note_section_loaded(pos);
            self.refresh_block_entity_index(pos);
            self.refresh_presented_index(pos);
            self.bump_terrain_revision();
        }
        true
    }

    pub(super) fn materialize_section_at(&mut self, c: IVec3) -> bool {
        match SectionPos::from_world(c.x, c.y, c.z) {
            Some(sp) => self.materialize_section(sp),
            None => false,
        }
    }

    #[inline]
    pub(in crate::world) fn column_gen(
        &self,
        pos: ChunkPos,
    ) -> Option<&Arc<petramond_worldgen::ColumnGen>> {
        self.side.server()?.gen.column_gen.get(&pos)
    }

    #[inline]
    pub(in crate::world) fn persisting(&self) -> bool {
        self.side.server().is_some_and(|s| s.save.is_some())
    }

    #[inline]
    pub(super) fn bump_terrain_revision(&mut self) {
        if let Some(server) = self.side.server_mut() {
            server.replication.bump_terrain_revision();
        }
    }
}

impl ServerWorld {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self::with_pool(seed, render_dist, Arc::new(JobPool::inline()))
    }

    pub fn with_pool(seed: u32, render_dist: i32, jobs: Arc<JobPool>) -> Self {
        let side = ServerSide::new(seed, render_dist, jobs.clone());
        Self::with_side(seed, render_dist, &jobs, side)
    }

    #[inline]
    pub(super) fn note_stream_nonfinal(&mut self, sp: SectionPos) {
        if self.data.stream_nonfinal.insert(sp) {
            self.side.replication.send_events.push(sp);
        }
    }

    #[inline]
    pub(super) fn settle_stream_nonfinal(&mut self, sp: SectionPos) {
        let gen = &self.side.gen;
        if !gen.pending_sections.contains(&sp)
            && !gen.awaited_overlays.contains(&sp)
            && !gen.pending_overlays.contains_key(&sp)
            && self.data.stream_nonfinal.remove(&sp)
        {
            self.side.replication.send_events.push(sp);
        }
    }

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
        self.side.replication.column_events.push(pos);
    }

    #[cfg(test)]
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

    #[inline]
    pub fn keep_inventory(&self) -> bool {
        self.side.session.keep_inventory
    }

    pub fn set_keep_inventory(&mut self, keep: bool) {
        self.side.session.keep_inventory = keep;
    }

    #[inline]
    pub fn day_cycle_ticks(&self) -> u64 {
        self.side.session.day_cycle_ticks
    }

    pub fn set_day_cycle_ticks(&mut self, ticks: u64) {
        self.side.session.day_cycle_ticks = ticks.max(1);
    }

    pub fn set_player_inputs(&mut self, inputs: Vec<super::session::PlayerInputSnapshot>) {
        self.side.session.player_inputs = inputs;
    }

    pub fn player_input(&self, player: u8) -> Option<super::session::PlayerInputSnapshot> {
        self.side
            .session
            .player_inputs
            .iter()
            .find(|i| i.id == player)
            .copied()
    }

    pub fn set_player_roster(&mut self, roster: Vec<super::session::PlayerRosterSnapshot>) {
        self.side.session.player_roster = roster;
    }

    pub fn player_roster(&self) -> &[super::session::PlayerRosterSnapshot] {
        &self.side.session.player_roster
    }

    pub fn worldgen_cache_report(&self) -> Vec<petramond_worldgen::cache::MemoStats> {
        self.side.gen.caches.report()
    }

    #[inline]
    pub fn terrain_revision(&self) -> u64 {
        self.side.replication.terrain_revision
    }

    #[inline]
    pub fn reach_budget(&self) -> &crate::mob::ReachBudget {
        &self.side.entities.nav_probe_budget
    }

    #[inline]
    pub(crate) fn kept_boxes(&self) -> &crate::mob::KeptBoxes {
        &self.side.entities.kept_boxes
    }

    #[inline]
    pub fn route_probe_budget(&self) -> &crate::mob::ReachBudget {
        &self.side.entities.route_probe_budget
    }
}

impl ReplicaWorld {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self::with_pool(seed, render_dist, Arc::new(JobPool::inline()))
    }

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
