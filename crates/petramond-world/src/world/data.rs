//! The deterministic world half: [`WorldData`] — sections, columns, indexes,
//! tick state, and every query/mutation that stays inside the data layer.
//!
//! The orchestration wrapper (`World` in the engine crate) owns job pools,
//! streaming, replication, and entity stores, and derefs here.

use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::{self, ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};
use crate::column::Column;
use crate::section::{Section, SectionSummary};

use super::environment::WorldEnvironment;
use super::load_targets::LoadTarget;
use super::saved_index::SavedIndex;
use super::tick_state::TickState;

/// Which half of the client/server split this `World` instance plays.
/// DEV TOOLING ONLY uses [`Combined`](WorldRole::Combined):
/// one world runs the sim AND meshes for the renderer.
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

pub struct ModSimState {
    /// Behavior hooks fired on mod-behavior blocks this tick (see
    /// `block::behavior::wasm`), in fire order. Drained by the game right
    /// after the world tick and dispatched to the owning mods; only blocks
    /// whose rows declare a `mod_id:name` behavior ever enqueue here.
    pub mod_block_hooks: Vec<crate::block::behavior::ModBlockHook>,
    /// Persistent mod world KV (`mod_id:key` → bytes) — the cross-mod interop
    /// surface. BTreeMap so the save encoding (it
    /// rides `level.dat`) iterates in one deterministic order. Mutated on the
    /// tick only (mod HostCalls); restored at session open.
    pub mod_kv: BTreeMap<String, Vec<u8>>,
    /// Mod pack ids DISABLED for this world (per-world `settings.json`; empty
    /// = all enabled). Session-fixed, set once at open; the natural spawner
    /// and the mod-set record consult it. The palette/mod-host gates take it
    /// separately at session construction.
    pub disabled_mods: std::collections::BTreeSet<String>,
    /// Per-cell baked SIM collision boxes for custom shapes — the sim
    /// bake cache the collision facet reads (a miss falls back to the row's
    /// static boxes, the failure policy). NOT persisted; re-baked from the pack
    /// WASM on load/edit. Boxes are content-interned to `'static` so
    /// `collision_boxes_at` keeps its `&'static` return (bounded by the shape's
    /// distinct configurations, not by cell count). See `world::custom_bake`.
    pub custom_bake: FxHashMap<crate::mathh::IVec3, &'static [crate::block::Aabb]>,
    /// Custom-shape cells needing a (re)bake — placed/edited since the last bake
    /// pump run. The host drains this, dispatches the shape's WASM bake, and
    /// fills `custom_bake`; a cell not in either falls back to its static boxes.
    pub custom_bake_dirty: FxHashSet<crate::mathh::IVec3>,
}

/// Mod-owned STREAMING/DRAW state: retained per-block draw sets and the
/// stream-event queue packs subscribe to. Presentation/orchestration — lives
/// on `World`, not [`WorldData`].
/// LAYERING CONTRACT: `WorldData` (and every `impl WorldData` file) must never
/// reference mesh, camera, atlas, net, server, save, worldgen, worker, mob,
/// entity, player, modding, or render. Orchestration state that needs those —
/// streaming jobs, mesh queues, replication, the session roster, the mob and
/// dropped-item stores, the save handle — lives on `World`, which wraps
/// this and derefs to it.
pub struct WorldData {
    pub seed: u32,
    /// Client/server role (see [`WorldRole`]); fixed at construction.
    pub role: WorldRole,
    /// Loaded section voxel data. Private to the `world` module: every external
    /// mutation routes through an accessor (`set_block_world`, the dirty-mesh queue)
    /// so the queue stays the single source of truth for what needs remeshing.
    ///
    /// Stored behind `Arc` so the off-thread light and mesh pools can take a cheap shared
    /// handle to a section (and its neighbours) instead of the render thread deep-copying it
    /// per bake — assembling those neighbourhoods was a multi-millisecond per-frame spike
    /// while streaming. Mutation is copy-on-write via [`Arc::make_mut`]: a setter clones a
    /// section's storage only while a bake still holds the old handle.
    pub sections: FxHashMap<SectionPos, Arc<Section>>,
    /// Per-column 2D data (biome, visible surface, direct-sky cover) shared by a
    /// vertical stack of sections. Cheap; ensured present whenever a section in
    /// the column loads.
    pub columns: FxHashMap<ChunkPos, Column>,
    /// Per-column presentation revision (biome/surface/sky-cover/summaries).
    /// Terrain replication resends ColumnData only when this
    /// changes, and revision-gated surface sampling relies on EQUALITY:
    /// values come from `column_revision_counter`, so a value is never reused
    /// — not even by a column that unloads and reloads with other content.
    pub column_payload_revisions: FxHashMap<ChunkPos, u64>,
    /// Store-wide source of unique column payload revision values.
    pub column_revision_counter: u64,
    /// Per-column bitset of *loaded* section `cy` values. Maintained at every
    /// section install/evict so planners (terrain send) iterate real stacks
    /// instead of probing the full vertical world range per wanted column.
    pub section_column_cys: FxHashMap<ChunkPos, u32>,
    /// Per-column bitset of loaded section `cy` values that hold at least one
    /// RANDOM-TICKABLE cell — the random-tick scan's working set. A derived
    /// index over `sections`: `random_tick_dirty` names the sections whose bit
    /// may be stale (any `section_mut` handout, any install), and the scan
    /// repairs those before it walks. Kept separate from
    /// [`section_column_cys`](WorldData::section_column_cys) so an underground
    /// stack of plain stone costs the scan nothing at all.
    pub section_column_rt: FxHashMap<ChunkPos, u32>,
    /// Sections whose `section_column_rt` bit may
    /// be stale. Bounded by the loaded section count (it is a set).
    pub random_tick_dirty: FxHashSet<SectionPos>,
    pub render_dist: i32,
    pub lighting_revision: u64,
    /// Sections currently holding at least one chest, door, or furnace, so the
    /// per-frame chest/door collection and the furnace tick visit only those
    /// sections instead of scanning every loaded one (mirrors `mesh_columns`).
    /// Maintained by `refresh_block_entity_index`
    /// at every install/mutation point; may briefly over-approximate (an indexed
    /// section whose last entity was cleared by a raw block edit costs one
    /// `is_empty` check), never under-approximate.
    pub block_entity_sections: FxHashSet<SectionPos>,
    /// Sections currently holding at least one block-row particle emitter. Kept separate
    /// from `block_entity_sections` so torch-heavy scenes do not make chest/door/furnace
    /// collection visit unrelated sections.
    pub particle_emitter_sections: FxHashSet<SectionPos>,
    /// Freshly streamed sections that have never produced light or a mesh, parked
    /// until their generation neighbourhood settles (`gen_neighborhood_settled`) so
    /// their FIRST bake and mesh run once, not once per landing neighbour. Without
    /// this, contiguous streaming rebaked/remeshed each section many times (each
    /// ingest dirtied its whole 3×3×3).
    pub light_deferred: FxHashSet<SectionPos>,
    /// A topology change may have made deferred first meshes ready. This keeps
    /// the O(deferred) settle scan off idle 200 Hz server pumps.
    pub deferred_recheck_needed: bool,
    /// Deferred centres whose 3x3x3 dependency changed since their last check.
    /// Ordinary ingest drains only these; a target reshape uses the full flag.
    pub deferred_rechecks: FxHashSet<SectionPos>,
    pub last_load_target: Option<LoadTarget>,
    /// Anchors beyond the first under multi-anchor streaming
    /// (`World::update_load_multi`); empty in single-anchor mode, so every
    /// single-anchor path is byte-identical to before. `last_load_target`
    /// stays the PRIMARY anchor (the priority/fallback target).
    pub extra_load_targets: Vec<LoadTarget>,
    /// The last missing-column scan found nothing left to request (everything
    /// wanted is loaded or pending), so the per-pump rescan can be skipped —
    /// with static anchors that scan is the entire steady-state streaming
    /// cost. Cleared by anything that can make a wanted column missing again:
    /// an anchor-set change, a column eviction, or a failed/discarded column
    /// gen job (see `poll_inner`).
    pub missing_columns_settled: bool,
    /// Each loaded column's per-cy `SectionSummary`s for ABSENT sections — the
    /// occupancy facts physics/placement read without loading (or generating)
    /// the section. On a server/combined world the streamer derives it once
    /// when the column's gen data lands; on a replica it arrives in the
    /// server's `ColumnPayload`. Indexed `cy - SECTION_MIN_CY`.
    pub column_summaries: FxHashMap<ChunkPos, Box<[SectionSummary]>>,
    /// Replica-only tint halos and deep-band floors carried by ColumnPayload.
    /// Combined/server worlds read the same facts from `column_gen`.
    pub column_biome_halos: FxHashMap<ChunkPos, Arc<[u8]>>,
    pub column_deep_band_los: FxHashMap<ChunkPos, i32>,
    /// Sections whose light went dirty since the last
    /// `pump_light_bakes` drain: the mark choke
    /// point feeds this set and the light pump requests from it. This is the
    /// demand path that does not depend on any mesh being queued — a distant
    /// sky-cover segment relights without pre-marking meshes (the landed
    /// bake's diff decides those), and headless servers have no mesh pump at
    /// all. Nearby sections are usually ALSO demanded by the mesh pump's
    /// `request_light_dependencies`; the pending-bake dedup makes that free.
    /// Bounded by edits per tick.
    pub relight_demand: FxHashSet<SectionPos>,
    /// Sections whose bake landed since the last save flush. Light changes
    /// don't set `modified` (they're derived, not player content), but a
    /// section whose on-disk record already exists must re-persist after a
    /// relight or its saved cubes go permanently stale (persisted light is
    /// only load-skippable because disk content is mutually consistent).
    /// Cleared wholesale by `flush_modified_chunks`. Empty without a save.
    pub relit_since_persist: FxHashSet<SectionPos>,
    /// Sections whose baked light a CONTENT change dirtied while a save is
    /// attached — their on-disk cubes are now pre-edit stale. Resolved by the
    /// rebake landing (`pump_light_bakes` moves them to `relit_since_persist`)
    /// or, if eviction/quit wins the race, by the persist gate rewriting the
    /// record WITHOUT light so reload rebakes instead of loading a permanent
    /// dark seam. Streaming-landing dirt is deliberately NOT tracked: those
    /// records stay mutually consistent on disk.
    pub light_edited_since_persist: FxHashSet<SectionPos>,
    /// Fixed-timestep simulation state: block updates + scheduled block ticks.
    pub sim: TickState,
    /// Sim-owned visual shader parameters.
    /// Mutated on the tick only (mod HostCalls); NOT persisted — resets to
    /// defaults on world open, the owning mod re-applies it (mod world KV).
    pub environment: WorldEnvironment,
    /// Mod-owned sim state (hooks, KV, bakes).
    pub mods: ModSimState,
    /// Sections whose streamed content is NOT final: an in-flight gen job, an
    /// unanswered saved-record request, or a pending saved overlay. The UNION
    /// of `WorldgenJobs`' three in-flight sets, maintained beside them by the
    /// streaming code through `World::note_stream_nonfinal` /
    /// `World::settle_stream_nonfinal` — the A-side fact behind
    /// [`stream_writable`](WorldData::stream_writable), so the deterministic
    /// half can guard writes without reaching into the job tables.
    pub stream_nonfinal: FxHashSet<SectionPos>,
    /// Which sections have on-disk save records (see [`SavedIndex`]). Built by
    /// `save::open_at`, attached with the save handle, mutated only by the
    /// save layer's write/delete choke points. Empty without a save.
    pub saved: SavedIndex,
}

impl WorldData {
    /// Ensure the per-column data for `(cx,cz)` exists, building it cheaply if not.
    /// Worldgen fills biome + both height maps; an empty column is the pre-gen placeholder.
    pub fn ensure_column(&mut self, pos: ChunkPos) -> &mut Column {
        if !self.column_payload_revisions.contains_key(&pos) {
            self.column_revision_counter += 1;
            self.column_payload_revisions
                .insert(pos, self.column_revision_counter);
        }
        self.columns.entry(pos).or_default()
    }

    /// Mod pack ids disabled for this world (per-world `settings.json`).
    #[inline]
    pub fn disabled_mods(&self) -> &std::collections::BTreeSet<String> {
        &self.mods.disabled_mods
    }

    /// Install the world's disabled-mod set — once, at session open.
    pub fn set_disabled_mods(&mut self, disabled: std::collections::BTreeSet<String>) {
        self.mods.disabled_mods = disabled;
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
        self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))
    }

    // --- World-coordinate routing ----------------------------------------------

    /// The one world-coordinate router: decode a world voxel `(wx, wy, wz)` into its
    /// owning [`SectionPos`] and section-local coords `(lx, ly, lz)` (each `0..16`),
    /// or `None` when `wy` falls outside the world vertical range. Section lookup is
    /// a separate step (see [`chunk_at_world`](Self::chunk_at_world)).
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

    /// The loaded section owning world voxel `(wx, wy, wz)` plus its section-local
    /// coords, or `None` if `wy` is out of range or the section is not loaded. The
    /// shared front end for every read-side world-coordinate accessor. (Named
    /// `chunk_at_world` for continuity; the unit it returns is now a [`Section`].)
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

    /// Mutable counterpart of [`chunk_at_world`](Self::chunk_at_world).
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

    /// The loaded section at `pos` — the cursor's raw resolve.
    #[inline]
    pub fn section_ref(&self, pos: SectionPos) -> Option<&Section> {
        self.sections.get(&pos).map(|s| &**s)
    }

    #[inline]
    pub fn section_mut(&mut self, pos: SectionPos) -> Option<&mut Section> {
        // Only a successful handout may change what the section holds. Marking
        // a failed lookup polluted the derived index with absent (and, for
        // boundary probes, out-of-range) positions; the next random-tick repair
        // then tried to encode those positions in the fixed-height bitset.
        let section = self.sections.get_mut(&pos)?;
        self.random_tick_dirty.insert(pos);
        Some(Arc::make_mut(section))
    }

    /// Whether the section owning world `(wx,wy,wz)` is loaded.
    #[inline]
    pub fn section_loaded_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        SectionPos::from_world(wx, wy, wz).is_some_and(|p| self.sections.contains_key(&p))
    }

    /// Cheap occupancy fact for a section, even when the voxel buffer has not been
    /// materialized. Loaded sections answer from exact counters. Unloaded generated
    /// sections answer from their column's absent-section summaries (derived when
    /// the column's gen data lands, or shipped in the server's ColumnPayload),
    /// unless a saved overlay could replace the generated base.
    pub fn section_summary(&self, pos: SectionPos) -> SectionSummary {
        if !SectionPos::cy_in_range(pos.cy) {
            return SectionSummary::Unknown;
        }
        if let Some(section) = self.sections.get(&pos) {
            return section.summary();
        }
        if self.saved_section_contains(pos) {
            return SectionSummary::Unknown;
        }
        if let Some(sums) = self.column_summaries.get(&pos.chunk_pos()) {
            let idx = (pos.cy - SECTION_MIN_CY) as usize;
            return sums.get(idx).copied().unwrap_or(SectionSummary::Unknown);
        }
        SectionSummary::Unknown
    }

    /// Whether `sp` may be written (or materialized) right now: no in-flight
    /// gen job and no in-flight saved overlay (see
    /// [`stream_nonfinal`](WorldData::stream_nonfinal)). A write into a
    /// pending-gen section would be clobbered by the landing result; a write
    /// into a section whose overlay is still in flight mutates content about
    /// to be replaced by the player's saved record.
    #[inline]
    pub fn stream_writable(&self, sp: SectionPos) -> bool {
        !self.stream_nonfinal.contains(&sp)
    }

    /// Whether `pos` has an on-disk save record (either store) — the absent
    /// section may not be trusted or materialized from generated facts.
    #[inline]
    pub fn saved_section_contains(&self, pos: SectionPos) -> bool {
        self.saved.contains(pos)
    }

    /// The on-disk record index (see [`SavedIndex`]).
    #[inline]
    pub fn saved_index(&self) -> &SavedIndex {
        &self.saved
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
        self.mods.mod_block_hooks.push(hook);
    }

    /// Drain the mod-behavior hooks fired this tick, in fire order.
    pub fn take_mod_block_hooks(&mut self) -> Vec<crate::block::behavior::ModBlockHook> {
        std::mem::take(&mut self.mods.mod_block_hooks)
    }

    /// All section coordinates of column `(cx,cz)` in the world vertical range.
    /// Concrete `RangeInclusive` (not `impl Iterator`) so callers can `.rev()` it.
    pub fn column_section_range() -> std::ops::RangeInclusive<i32> {
        SECTION_MIN_CY..=SECTION_MAX_CY
    }
}
