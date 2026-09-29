use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{self, ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};
use crate::column::Column;
use crate::section::{Section, SectionSummary};

use super::environment::WorldEnvironment;
use super::load_targets::LoadTarget;
use super::saved_index::SavedIndex;
use super::section_map::SectionMap;
use super::tick_state::TickState;

#[derive(Default)]
pub struct ContentState {
    pub block_hooks: Vec<crate::block::behavior::BlockHook>,
    pub world_kv: BTreeMap<String, Vec<u8>>,
    pub disabled_mods: std::collections::BTreeSet<String>,
    pub custom_bake: FxHashMap<crate::mathh::IVec3, &'static [crate::block::Aabb]>,
    pub custom_bake_dirty: FxHashSet<crate::mathh::IVec3>,
}

pub struct WorldData {
    pub seed: u32,
    /// Loaded section voxel data. Private to the `world` module: every external
    /// mutation routes through an accessor (`set_block_world`, the dirty-mesh queue)
    /// so the queue stays the single source of truth for what needs remeshing.
    ///
    /// Stored behind `Arc` so the off-thread light and mesh pools can take a cheap shared
    /// handle to a section (and its neighbours) instead of the render thread deep-copying it
    /// per bake — assembling those neighbourhoods was a multi-millisecond per-frame spike
    /// while streaming. Mutation is copy-on-write via [`Arc::make_mut`]: a setter clones a
    /// section's storage only while a bake still holds the old handle.
    pub sections: SectionMap,
    pub columns: FxHashMap<ChunkPos, std::sync::Arc<Column>>,
    pub column_payload_revisions: FxHashMap<ChunkPos, u64>,
    pub column_revision_counter: u64,
    pub section_column_cys: FxHashMap<ChunkPos, u32>,
    pub random_tick_index: super::random_tick_index::RandomTickIndex,
    /// Sections mutably touched or installed since the random-tick index last absorbed them.
    pub random_tick_dirty: FxHashSet<SectionPos>,
    /// Sections touched since the last save flush: the only ones whose persisted state can
    /// have changed beyond the light and entity sets the flush also reads.
    pub persist_candidates: FxHashSet<SectionPos>,
    pub render_dist: i32,
    pub lighting_revision: u64,
    pub block_entity_sections: FxHashSet<SectionPos>,
    pub particle_emitter_sections: FxHashSet<SectionPos>,
    /// Freshly streamed sections that have never produced light or a mesh, parked
    /// until their generation neighbourhood settles (`gen_neighborhood_settled`) so
    /// their FIRST bake and mesh run once, not once per landing neighbour. Without
    /// this, contiguous streaming rebaked/remeshed each section many times (each
    /// ingest dirtied its whole 3×3×3).
    pub light_deferred: FxHashSet<SectionPos>,
    pub deferred_recheck_needed: bool,
    pub deferred_rechecks: FxHashSet<SectionPos>,
    pub last_load_target: Option<LoadTarget>,
    pub extra_load_targets: Vec<LoadTarget>,
    pub missing_columns_settled: bool,
    pub column_summaries: FxHashMap<ChunkPos, Box<[SectionSummary]>>,
    pub column_biome_halos: FxHashMap<ChunkPos, Arc<[u8]>>,
    pub column_deep_band_los: FxHashMap<ChunkPos, i32>,
    pub relight_demand: FxHashSet<SectionPos>,
    pub light_edits: Vec<(crate::mathh::IVec3, i32)>,
    pub relit_since_persist: FxHashSet<SectionPos>,
    pub light_edited_since_persist: FxHashSet<SectionPos>,
    pub sim: TickState,
    pub environment: WorldEnvironment,
    pub content: ContentState,
    pub stream_nonfinal: FxHashSet<SectionPos>,
    pub saved: SavedIndex,
}

impl WorldData {
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self {
            seed,
            sections: SectionMap::default(),
            columns: FxHashMap::default(),
            column_payload_revisions: FxHashMap::default(),
            column_revision_counter: 0,
            section_column_cys: FxHashMap::default(),
            random_tick_index: Default::default(),
            random_tick_dirty: FxHashSet::default(),
            persist_candidates: FxHashSet::default(),
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
            light_edits: Vec::new(),
            relit_since_persist: FxHashSet::default(),
            light_edited_since_persist: FxHashSet::default(),
            sim: TickState::new(seed),
            environment: WorldEnvironment::default(),
            content: ContentState::default(),
            stream_nonfinal: FxHashSet::default(),
            saved: SavedIndex::default(),
        }
    }

    pub fn ensure_column(&mut self, pos: ChunkPos) -> &mut Column {
        if !self.column_payload_revisions.contains_key(&pos) {
            self.column_revision_counter += 1;
            self.column_payload_revisions
                .insert(pos, self.column_revision_counter);
        }
        Arc::make_mut(self.columns.entry(pos).or_default())
    }

    #[inline]
    pub fn disabled_mods(&self) -> &std::collections::BTreeSet<String> {
        &self.content.disabled_mods
    }

    pub fn set_disabled_mods(&mut self, disabled: std::collections::BTreeSet<String>) {
        self.content.disabled_mods = disabled;
    }

    pub fn environment(&self) -> &WorldEnvironment {
        &self.environment
    }

    pub fn set_shader_param(&mut self, key: String, value: [f32; 4]) {
        self.environment.set_shader_param(key, value);
    }

    #[inline]
    pub fn lighting_revision(&self) -> u64 {
        self.lighting_revision
    }

    pub fn bump_lighting_revision(&mut self) {
        self.lighting_revision = self.lighting_revision.wrapping_add(1);
    }

    pub fn bump_column_payload_revision(&mut self, pos: ChunkPos) {
        self.column_revision_counter += 1;
        self.column_payload_revisions
            .insert(pos, self.column_revision_counter);
    }

    pub fn column_payload_revision(&self, pos: ChunkPos) -> u64 {
        self.column_payload_revisions
            .get(&pos)
            .copied()
            .unwrap_or(0)
    }

    #[inline]
    pub fn column_at(&self, wx: i32, wz: i32) -> Option<&Column> {
        self.columns
            .get(&ChunkPos::new(wx >> 4, wz >> 4))
            .map(|c| &**c)
    }

    #[inline]
    pub fn split_world(wx: i32, wy: i32, wz: i32) -> Option<(SectionPos, usize, usize, usize)> {
        let sp = SectionPos::from_world(wx, wy, wz)?;
        Some((
            sp,
            chunk::lx(wx),
            wy.rem_euclid(SECTION_SIZE as i32) as usize,
            chunk::lz(wz),
        ))
    }

    #[inline]
    pub fn chunk_at_world(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(&Section, usize, usize, usize)> {
        let (pos, lx, ly, lz) = WorldData::split_world(wx, wy, wz)?;
        let s = self.sections.get(&pos)?;
        Some((s, lx, ly, lz))
    }

    #[inline]
    pub fn chunk_at_world_mut(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(&mut Section, usize, usize, usize)> {
        let (pos, lx, ly, lz) = WorldData::split_world(wx, wy, wz)?;
        let s = self.section_mut(pos)?;
        Some((s, lx, ly, lz))
    }

    #[inline]
    pub fn section_ref(&self, pos: SectionPos) -> Option<&Section> {
        self.sections.get(&pos).map(|s| &**s)
    }

    #[inline]
    pub fn section_mut(&mut self, pos: SectionPos) -> Option<&mut Section> {
        let section = self.sections.get_mut(&pos)?;
        self.random_tick_dirty.insert(pos);
        Some(Arc::make_mut(section))
    }

    #[inline]
    pub fn section_loaded_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        SectionPos::from_world(wx, wy, wz).is_some_and(|p| self.sections.contains_key(&p))
    }

    pub fn section_summary(&self, pos: SectionPos) -> SectionSummary {
        if !SectionPos::cy_in_range(pos.cy) {
            return SectionSummary::Unknown;
        }
        if let Some(section) = self.sections.get(&pos) {
            return section.summary();
        }
        self.unloaded_section_summary(pos)
    }

    pub fn unloaded_section_summary(&self, pos: SectionPos) -> SectionSummary {
        if !SectionPos::cy_in_range(pos.cy) || self.saved_section_contains(pos) {
            return SectionSummary::Unknown;
        }
        if let Some(sums) = self.column_summaries.get(&pos.chunk_pos()) {
            let idx = (pos.cy - SECTION_MIN_CY) as usize;
            return sums.get(idx).copied().unwrap_or(SectionSummary::Unknown);
        }
        SectionSummary::Unknown
    }

    #[inline]
    pub fn stream_writable(&self, sp: SectionPos) -> bool {
        !self.stream_nonfinal.contains(&sp)
    }

    #[inline]
    pub fn saved_section_contains(&self, pos: SectionPos) -> bool {
        self.saved.contains(pos)
    }

    #[inline]
    pub fn saved_index(&self) -> &SavedIndex {
        &self.saved
    }

    pub fn physics_block(&self, wx: i32, wy: i32, wz: i32) -> Block {
        if !crate::border::contains_column(wx, wz) {
            return Block::Stone;
        }
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

    pub fn fluid_cell_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        self.physics_block(wx, wy, wz).fluid().is_some()
    }

    pub fn mark_chunk_modified(&mut self, pos: crate::mathh::IVec3) {
        if let Some((s, ..)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
            s.modified = true;
        }
    }

    pub fn queue_block_hook(&mut self, hook: crate::block::behavior::BlockHook) {
        self.content.block_hooks.push(hook);
    }

    pub fn take_block_hooks(&mut self) -> Vec<crate::block::behavior::BlockHook> {
        std::mem::take(&mut self.content.block_hooks)
    }

    pub fn column_section_range() -> std::ops::RangeInclusive<i32> {
        SECTION_MIN_CY..=SECTION_MAX_CY
    }
}

pub fn revision_base() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1 << 40, Ordering::Relaxed)
}
