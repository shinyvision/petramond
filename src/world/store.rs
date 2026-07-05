use std::collections::{BTreeMap, HashMap, HashSet};
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
use crate::worker::{JobPool, WorkerPool};
use crate::worldgen::driver::ChunkGenerator;
use crate::worldgen::driver::ColumnGen;

use super::entities::DroppedItems;
use super::environment::WorldEnvironment;
use super::light::LightBakeQueue;
use super::mesh_queue::DirtyMeshQueue;
use super::tick::TickState;

pub const RENDER_DIST: i32 = 16;

/// Vertical load radius (in 16³ sections) around the player's section: the world
/// streams a flattened cylinder — a Euclidean horizontal disc of columns × this many
/// sections above and below the player. Sized so the visible surface band is fully
/// loaded when standing on typical terrain, while the deep underground / high sky a
/// far column doesn't need is left ungenerated until the player approaches it (the
/// per-section "generate closest to the player" payoff that makes room for caves).
pub const VERTICAL_LOAD_RADIUS: i32 = 5;
pub(super) const OMNI_LOAD_RADIUS: i32 = 5;
pub(super) const FORWARD_LOAD_DOT_MIN: f32 = -0.15;

const TERRAIN_PRIORITY_SCALE: i64 = 1024;
const VIEW_PRIORITY_FRONT_DOT_MIN: f32 = 0.5;
const VIEW_PRIORITY_SIDE_PENALTY: i64 = 192;
const VIEW_PRIORITY_SOFT_CONE_PENALTY: i64 = 256;
const VIEW_PRIORITY_BEHIND_PENALTY: i64 = 768;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct LoadTarget {
    pub center: ChunkPos,
    /// Player's section `cy` — the centre of the vertical load window.
    pub center_cy: i32,
    pub render_dist: i32,
    /// Quantized horizontal camera direction. `None` means legacy/full-disc loading.
    pub view_sector: Option<i8>,
}

impl LoadTarget {
    pub fn new(cx: i32, cy: i32, cz: i32, render_dist: i32) -> Self {
        Self {
            center: ChunkPos::new(cx, cz),
            center_cy: cy,
            render_dist,
            view_sector: None,
        }
    }

    pub fn new_facing(
        cx: i32,
        cy: i32,
        cz: i32,
        render_dist: i32,
        forward_x: f32,
        forward_z: f32,
    ) -> Self {
        const SECTORS: i32 = 16;
        let len2 = forward_x * forward_x + forward_z * forward_z;
        let view_sector = if len2 > 0.0001 {
            let angle = forward_x.atan2(forward_z).rem_euclid(std::f32::consts::TAU);
            Some(
                ((angle / std::f32::consts::TAU * SECTORS as f32).round() as i32)
                    .rem_euclid(SECTORS) as i8,
            )
        } else {
            None
        };
        Self {
            center: ChunkPos::new(cx, cz),
            center_cy: cy,
            render_dist,
            view_sector,
        }
    }

    pub fn view_dir(self) -> Option<(f32, f32)> {
        const SECTORS: f32 = 16.0;
        let sector = self.view_sector? as f32;
        let angle = sector / SECTORS * std::f32::consts::TAU;
        Some((angle.sin(), angle.cos()))
    }

    fn view_priority_penalty(self, dx: i32, dz: i32) -> i64 {
        let Some((fx, fz)) = self.view_dir() else {
            return 0;
        };
        let d2 = dx * dx + dz * dz;
        if d2 == 0 || d2 <= OMNI_LOAD_RADIUS * OMNI_LOAD_RADIUS {
            return 0;
        }
        let dist = (d2 as f32).sqrt();
        let forward_dot = (dx as f32 * fx + dz as f32 * fz) / dist;
        let side = ((dx as f32 * fz - dz as f32 * fx).abs() / dist).clamp(0.0, 1.0);
        let cone_penalty = if forward_dot >= VIEW_PRIORITY_FRONT_DOT_MIN {
            0
        } else if forward_dot >= FORWARD_LOAD_DOT_MIN {
            VIEW_PRIORITY_SOFT_CONE_PENALTY
        } else {
            VIEW_PRIORITY_BEHIND_PENALTY
        };
        cone_penalty + (side * VIEW_PRIORITY_SIDE_PENALTY as f32) as i64
    }

    pub(super) fn column_priority_key(self, pos: ChunkPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dz = pos.cz - self.center.cz;
        let d2 = (dx as i64 * dx as i64) + (dz as i64 * dz as i64);
        d2 * TERRAIN_PRIORITY_SCALE + self.view_priority_penalty(dx, dz)
    }

    pub(super) fn section_priority_key(self, pos: SectionPos) -> i64 {
        let dx = pos.cx - self.center.cx;
        let dy = pos.cy - self.center_cy;
        let dz = pos.cz - self.center.cz;
        let d2 = (dx as i64 * dx as i64) + (dy as i64 * dy as i64) + (dz as i64 * dz as i64);
        d2 * TERRAIN_PRIORITY_SCALE + self.view_priority_penalty(dx, dz)
    }
}

/// The cubic voxel world: a sparse 3D grid of 16³ [`Section`]s plus a sparse 2D
/// grid of per-column [`Column`] data (biome, surface heightmap). Sections are the
/// unit of storage, meshing, lighting, streaming, and saving; a column exists
/// whenever any of its sections is loaded (see [`ensure_column`](World::ensure_column)).
pub struct World {
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
    pub(super) sections: HashMap<SectionPos, Arc<Section>>,
    /// Per-column 2D data (biome, surface heightmap) shared by a vertical stack of
    /// sections. Cheap; ensured present whenever a section in the column loads.
    pub(super) columns: HashMap<ChunkPos, Column>,
    /// One GPU-ready mesh per section.
    pub(super) meshes: HashMap<SectionPos, ChunkMesh>,
    /// XZ columns that currently have at least one CPU section mesh.
    /// Mirrors `meshes` so renderer retention does not scan the vertical range
    /// of every GPU column each frame.
    pub(super) mesh_columns: HashSet<ChunkPos>,
    /// XZ columns whose packed render buffer must be rebuilt from `meshes`.
    /// Kept explicitly so the renderer does not scan every section mesh each frame.
    pub(super) mesh_upload_dirty_columns: HashSet<ChunkPos>,
    pub worker: WorkerPool,
    /// Columns whose shared 2D gen data (`ColumnGen`) has landed: the source for
    /// submitting per-section jobs and sizing each column's vertical load window.
    /// Present for every loaded column; dropped when the column unloads.
    pub(super) column_gen: HashMap<ChunkPos, Arc<ColumnGen>>,
    /// Columns queued for the (heavy, once-per-column) `ColumnGen` job.
    pub(super) pending: HashMap<ChunkPos, ()>,
    /// Sections with an in-flight per-section gen job, so the streamer never submits a
    /// section twice while it is being generated.
    pub(super) pending_sections: HashSet<SectionPos>,
    /// Saved (player-modified) sections read back from disk whose generated column has
    /// not arrived yet — disk I/O usually beats noise-gen. Held here until the column
    /// lands, then overlaid over the generated terrain (see `world::stream::poll`).
    pub(super) pending_overlays: HashMap<SectionPos, super::stream::LoadedOverlay>,
    /// Sections whose saved record has been REQUESTED from the save thread but not
    /// answered yet. Until the answer lands (and any overlay applies) the section's
    /// true content is in flight: the sim guard blocks mutation and the harvest skips
    /// persisting it (see `world::sim_guard`).
    pub(super) awaited_overlays: HashSet<SectionPos>,
    pub render_dist: i32,
    pub(super) lighting_revision: u64,
    pub(super) light_bakes: LightBakeQueue,
    /// Off-thread section meshing: dirty sections are submitted as owned snapshots and
    /// finished meshes drained back, so the render thread never builds a mesh.
    pub(super) mesh_pool: super::mesh_pool::MeshPool,
    pub(super) mesh_jobs_in_flight: usize,
    pub(super) dirty_meshes: DirtyMeshQueue,
    /// Loaded sections wholly below their column's surface retention band — only
    /// visible through cave openings (see `world::visibility`).
    pub(super) deep_sections: HashSet<SectionPos>,
    /// The deep sections the last visibility refresh could reach from the visible
    /// region. Deep sections outside this set park instead of meshing.
    pub(super) visible_deep: HashSet<SectionPos>,
    /// Dirty deep sections parked because nothing can see them. Re-queued by the
    /// visibility refresh when they become reachable (or the player ring arrives).
    pub(super) hidden_parked: HashSet<SectionPos>,
    /// Raised by ingest / edits / load-target moves; consumed by the mesh pump,
    /// which re-runs the deep-visibility BFS before submitting work.
    pub(super) vis_dirty: bool,
    /// Dirty meshes parked while async light bakes their sampling neighbourhood.
    /// They re-enter `dirty_meshes` only once the 3×3×3 light dependency set is clean.
    pub(super) light_blocked_meshes: HashSet<SectionPos>,
    /// Freshly streamed sections that have never produced light or a mesh, parked
    /// until their generation neighbourhood settles (`gen_neighborhood_settled`) so
    /// their FIRST bake and mesh run once, not once per landing neighbour. Without
    /// this, contiguous streaming rebaked/remeshed each section many times (each
    /// ingest dirtied its whole 3×3×3).
    pub(super) light_deferred: HashSet<SectionPos>,
    pub(super) last_load_target: Option<LoadTarget>,
    /// Fixed-timestep simulation state: block updates + scheduled block ticks.
    pub(super) sim: TickState,
    /// On-disk save handle (`None` if saving is disabled / failed to open).
    pub(super) save: Option<WorldSave>,
    /// Active dropped item entities resting in currently-loaded sections.
    pub(super) dropped_items: DroppedItems,
    /// Active mobs in currently-loaded sections.
    pub(super) mobs: Mobs,
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
    /// surface (WIKI/modding.md Phase 3b). BTreeMap so the save encoding (it
    /// rides `level.dat`) iterates in one deterministic order. Mutated on the
    /// tick only (mod HostCalls); restored at session open.
    pub(super) mod_kv: BTreeMap<String, Vec<u8>>,
    /// Mod pack ids DISABLED for this world (per-world `settings.json`; empty
    /// = all enabled). Session-fixed, set once at open; the natural spawner
    /// and the mod-set record consult it. The palette/mod-host gates take it
    /// separately at session construction.
    pub(super) disabled_mods: std::collections::BTreeSet<String>,
    /// The open mod GUI session's state map (WIKI/modding.md Phase 5). Lives
    /// on the world so `GuiStateSet/Get` HostCalls reach it through `SimCtx`;
    /// the SESSION lifecycle (cleared on open/close) is driven by the game's
    /// menu funnel. Behind `Arc` so the per-frame UI snapshot is a refcount
    /// bump; tick-side writes are copy-on-write ([`std::sync::Arc::make_mut`]
    /// clones at most once per outstanding snapshot). NOT persisted.
    pub(super) gui_state: std::sync::Arc<crate::gui::GuiStateMap>,
}

impl World {
    pub fn new(seed: u32, render_dist: i32) -> Self {
        // ONE background pool shared by every streaming stage; the per-stage adapters
        // below each hold a handle and compete purely on distance priority.
        let jobs = std::sync::Arc::new(JobPool::new(JobPool::default_threads()));
        Self {
            seed,
            sections: HashMap::new(),
            columns: HashMap::new(),
            meshes: HashMap::new(),
            mesh_columns: HashSet::new(),
            mesh_upload_dirty_columns: HashSet::new(),
            worker: WorkerPool::new(jobs.clone()),
            column_gen: HashMap::new(),
            pending: HashMap::new(),
            pending_sections: HashSet::new(),
            pending_overlays: HashMap::new(),
            awaited_overlays: HashSet::new(),
            render_dist,
            lighting_revision: 0,
            light_bakes: LightBakeQueue::new(jobs.clone()),
            mesh_pool: super::mesh_pool::MeshPool::new(jobs),
            mesh_jobs_in_flight: 0,
            dirty_meshes: DirtyMeshQueue::default(),
            deep_sections: HashSet::new(),
            visible_deep: HashSet::new(),
            hidden_parked: HashSet::new(),
            vis_dirty: false,
            light_blocked_meshes: HashSet::new(),
            light_deferred: HashSet::new(),
            last_load_target: None,
            sim: TickState::new(seed),
            save: None,
            dropped_items: DroppedItems::default(),
            mobs: Mobs::new(seed as u64),
            stream_events: Vec::new(),
            stream_events_enabled: false,
            environment: WorldEnvironment::default(),
            mod_kv: BTreeMap::new(),
            disabled_mods: std::collections::BTreeSet::new(),
            gui_state: crate::gui::empty_gui_state(),
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

    /// Attach an on-disk save: enables section persistence (load-from-disk in the
    /// streamer and flush-on-evict) and gives `Game` a handle for level/entities.
    pub fn attach_save(&mut self, save: WorldSave) {
        self.save = Some(save);
    }

    pub fn save(&self) -> Option<&WorldSave> {
        self.save.as_ref()
    }

    pub fn save_mut(&mut self) -> Option<&mut WorldSave> {
        self.save.as_mut()
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
        if section.modified || !entities.is_empty() || !mobs.is_empty() || record_holds_entities {
            let mut snap = SectionSnapshot::from_section(section);
            snap.entities = entities;
            snap.mobs = mobs;
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
        // Post-action: a persisted section is now in sync with disk.
        for pos in persisted {
            if let Some(s) = self.section_mut(pos) {
                s.modified = false;
            }
        }
        if let Some(save) = self.save.as_mut() {
            save.save_sections(snaps);
        }
    }

    /// Recompute a column's surface heightmap from its currently-loaded sections,
    /// scanning each `(x,z)` from the top section down to the first non-air block. Used
    /// after overlaying a saved (player-modified) section, whose blocks can differ from
    /// what generation produced, so skylight and spawn see the true surface.
    pub(super) fn recompute_column_heightmap(&mut self, cpos: ChunkPos) {
        // Gather surfaces under immutable section borrows, then write the column once
        // (the section and column maps are distinct, but both borrow `self`).
        let mut surf = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut remaining = surf.len();
        for cy in Self::column_section_range().rev() {
            if remaining == 0 {
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
                    if surf[col] != NO_SURFACE {
                        continue; // a higher section already set this column's surface.
                    }
                    for ly in (0..SECTION_SIZE).rev() {
                        if blocks[section_idx(lx, ly, lz)] != 0 {
                            surf[col] = oy + ly as i32;
                            remaining -= 1;
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
                let scanned = surf[i];
                let ground_loaded = SectionPos::from_world(
                    cpos.cx * SECTION_SIZE as i32 + lx as i32,
                    ground,
                    cpos.cz * SECTION_SIZE as i32 + lz as i32,
                )
                .is_some_and(|sp| self.sections.contains_key(&sp));
                let h = if ground_loaded || ground == NO_SURFACE {
                    scanned
                } else if scanned == NO_SURFACE {
                    ground
                } else {
                    scanned.max(ground)
                };
                surf[i] = h;
            }
        }
        let col = self.ensure_column(cpos);
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                col.set_surface_y(lx, lz, surf[lz * SECTION_SIZE + lx]);
            }
        }
        // (No whole-column relight here: it queued all 20 sections — including deep,
        // enclosed ones — bypassing the mesh/light skip and flooding the streaming
        // backlog. Skylight after a surface shift is handled by the ingested section's own
        // 3×3×3 neighbourhood marking in `poll`, which covers the canopy/structure band.)
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

    /// Advance the mobs one fixed game tick against an immutable view of the rest of
    /// the world (the field is moved out so the `&mut Mobs` and `&World` borrows stay
    /// disjoint). Returns the melee strikes the mobs landed on the player this tick,
    /// for `Game` to apply through the player damage pipeline.
    pub fn tick_mobs(
        &mut self,
        dt: f32,
        player_pos: Vec3,
        player_body: Option<crate::mob::Body>,
    ) -> Vec<crate::mob::MobAttack> {
        if self.mobs.is_empty() {
            return Vec::new();
        }
        let freeze_unloaded = self.save.is_some();
        let mut mobs = std::mem::take(&mut self.mobs);
        let attacks = mobs.tick(dt, self, player_pos, player_body, freeze_unloaded);
        self.mobs = mobs;
        attacks
    }

    /// Run one natural mob-spawn attempt (one per game tick). Returns the mobs
    /// actually spawned, for the caller to report as `mob_spawned` events.
    pub fn spawn_mobs_tick(&mut self, player_pos: Vec3) -> Vec<(crate::mob::Mob, Vec3)> {
        let mut mobs = std::mem::take(&mut self.mobs);
        let spawned = mobs.spawn_tick(self, player_pos);
        self.mobs = mobs;
        spawned
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
        }
    }

    pub(super) fn queue_dirty_mesh(&mut self, pos: SectionPos) {
        if let Some(s) = self.section_mut(pos) {
            s.dirty = true;
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
            self.light_blocked_meshes.remove(&pos);
            self.hidden_parked.remove(&pos);
            self.dirty_meshes.push(pos);
        }
    }

    pub(super) fn mark_light_and_mesh_dirty_pos(&mut self, pos: SectionPos) {
        self.mark_light_dirty_pos(pos);
        self.queue_dirty_mesh(pos);
    }

    /// A column heightmap cell moved, changing which cells are considered open sky
    /// for every skylight bake whose 3×3 XZ seed grid includes this column. Dirty only
    /// sections already in memory; absent generated/sky sections will bake from the new
    /// heightmap when they stream in or materialize.
    pub(super) fn mark_heightmap_light_dirty_around(&mut self, center: ChunkPos) {
        let mut affected = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                for cy in Self::column_section_range() {
                    let pos = SectionPos::new(center.cx + dx, cy, center.cz + dz);
                    if self.sections.contains_key(&pos) {
                        affected.push(pos);
                    }
                }
            }
        }
        for pos in affected {
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
            if self.save.as_ref().is_some_and(|s| s.manifest_contains(pos)) {
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
        }
        true
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
    /// Worldgen fills biome + heightmap; an empty column is the pre-gen placeholder.
    pub(super) fn ensure_column(&mut self, pos: ChunkPos) -> &mut Column {
        self.columns.entry(pos).or_insert_with(Column::new)
    }

    #[inline]
    pub(super) fn column_at(&self, wx: i32, wz: i32) -> Option<&Column> {
        self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))
    }

    #[inline]
    pub(super) fn column_at_mut(&mut self, wx: i32, wz: i32) -> Option<&mut Column> {
        self.columns.get_mut(&ChunkPos::new(wx >> 4, wz >> 4))
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
        if self.save.as_ref().is_some_and(|s| s.manifest_contains(pos)) {
            return SectionSummary::Unknown;
        }
        self.column_gen
            .get(&pos.chunk_pos())
            .map_or(SectionSummary::Unknown, |col| col.section_summary(pos.cy))
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
        self.mesh_columns.insert(pos.chunk_pos());
        self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
    }

    pub(super) fn remove_mesh(&mut self, pos: SectionPos) -> bool {
        let removed = self.meshes.remove(&pos).is_some();
        if removed {
            self.refresh_mesh_column_presence(pos.chunk_pos());
        }
        removed
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

    pub(super) fn remove_section(&mut self, pos: SectionPos) {
        self.sections.remove(&pos);
        self.awaited_overlays.remove(&pos);
        if self.remove_mesh(pos) {
            self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
        }
        self.dirty_meshes.remove(pos);
        self.light_blocked_meshes.remove(&pos);
        self.light_deferred.remove(&pos);
        self.deep_sections.remove(&pos);
        self.visible_deep.remove(&pos);
        self.hidden_parked.remove(&pos);
        self.light_bakes.cancel(pos);
        self.mark_light_dirty_neighborhood(pos, false);
        self.mark_dirty_neighborhood(pos, false);
    }

    /// Evict an entire column: all its loaded sections, meshes, queues, per-column data,
    /// and any pending gen.
    pub(super) fn remove_column(&mut self, pos: ChunkPos) {
        for cy in Self::column_section_range() {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.sections.remove(&sp);
            self.meshes.remove(&sp);
            self.dirty_meshes.remove(sp);
            self.light_blocked_meshes.remove(&sp);
            self.light_deferred.remove(&sp);
            self.deep_sections.remove(&sp);
            self.visible_deep.remove(&sp);
            self.hidden_parked.remove(&sp);
            self.light_bakes.cancel(sp);
        }
        self.mesh_columns.remove(&pos);
        self.mesh_upload_dirty_columns.remove(&pos);
        self.columns.remove(&pos);
        self.column_gen.remove(&pos);
        self.pending.remove(&pos);
        self.pending_sections.retain(|sp| sp.chunk_pos() != pos);
        self.awaited_overlays.retain(|sp| sp.chunk_pos() != pos);
    }

    /// Drop all loaded sections, columns, meshes, and the in-flight gen set — the
    /// regen path.
    pub fn clear_world(&mut self) {
        self.sections.clear();
        self.deep_sections.clear();
        self.visible_deep.clear();
        self.hidden_parked.clear();
        self.columns.clear();
        self.column_gen.clear();
        self.meshes.clear();
        self.mesh_columns.clear();
        self.mesh_upload_dirty_columns.clear();
        self.light_blocked_meshes.clear();
        self.light_deferred.clear();
        self.pending.clear();
        self.pending_sections.clear();
        self.pending_overlays.clear();
        self.awaited_overlays.clear();
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
        self.queue_dirty_mesh(pos);
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
        for (cy, section) in sections {
            let sp = SectionPos::new(pos.cx, cy, pos.cz);
            self.sections.insert(sp, Arc::new(section));
            self.queue_dirty_mesh(sp);
        }
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
            self.queue_dirty_mesh(sp);
        }
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

        world.recompute_column_heightmap(cp);

        assert_eq!(
            world.columns.get(&cp).unwrap().surface_y(x, z),
            cave_top,
            "heightmap refresh must not restore original pre-cave surface {original}"
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
        world.ensure_column(cp).set_surface_y(x, z, ground);
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

        world.recompute_column_heightmap(cp);

        assert_eq!(
            world.columns.get(&cp).unwrap().surface_y(x, z),
            lower,
            "a loaded dug shaft must not be covered again by the generated fallback"
        );
    }

    #[test]
    fn removing_surface_cover_relights_loaded_sections_below_the_changed_section() {
        let mut world = World::new(0, 0);
        let cp = ChunkPos::new(0, 0);
        let shaft_x = 8;
        let shaft_z = 8;
        let cover_y = 64;
        let top = SectionPos::new(0, 4, 0);
        let lower = SectionPos::new(0, 2, 0);

        world
            .ensure_column(cp)
            .set_surface_y(shaft_x, shaft_z, cover_y);

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

        let lower = world.sections.get(&lower).unwrap();
        assert!(
            lower.light_dirty,
            "removing the heightmap cover must invalidate skylight below the edited section"
        );
        assert!(
            lower.dirty,
            "sections whose cached light can change must be remeshed after the rebake"
        );
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
