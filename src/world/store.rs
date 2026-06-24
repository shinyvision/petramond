use std::collections::HashMap;

use crate::chunk::{self, Chunk, ChunkPos, CHUNK_SY, SECTION_COUNT};
use crate::entity::DroppedItem;
use crate::mathh::Vec3;
use crate::mesh::ChunkMesh;
use crate::mob::{Mobs, SavedMob};
use crate::save::{ChunkSnapshot, WorldSave};
use crate::worker::WorkerPool;

use super::entities::DroppedItems;
use super::light_queue::LightBakeQueue;
use super::mesh_queue::DirtyMeshQueue;
use super::tick::TickState;
use super::visibility::SectionConnectivity;

pub const RENDER_DIST: i32 = 16;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct LoadTarget {
    pub center: ChunkPos,
    pub render_dist: i32,
}

impl LoadTarget {
    pub fn new(cx: i32, cz: i32, render_dist: i32) -> Self {
        Self {
            center: ChunkPos::new(cx, cz),
            render_dist,
        }
    }
}

pub struct World {
    pub seed: u32,
    /// Loaded chunk voxel data. Private to the `world` module: every external
    /// mutation routes through an accessor (`set_block_world`, `clear_world`,
    /// the dirty-mesh queue) so the queue stays the single source of truth for
    /// what needs remeshing — see `mesh_queue`.
    pub(super) chunks: HashMap<ChunkPos, Chunk>,
    pub(super) meshes: HashMap<ChunkPos, ChunkMesh>,
    pub worker: WorkerPool,
    /// Chunks queued for gen (waiting on result).
    pub(super) pending: HashMap<ChunkPos, ()>,
    pub render_dist: i32,
    pub(super) section_visibility: HashMap<ChunkPos, [SectionConnectivity; SECTION_COUNT]>,
    pub(super) visibility_revision: u64,
    pub(super) lighting_revision: u64,
    pub(super) light_bakes: LightBakeQueue,
    pub(super) dirty_meshes: DirtyMeshQueue,
    pub(super) last_load_target: Option<LoadTarget>,
    /// Fixed-timestep simulation state: block updates + scheduled block ticks.
    pub(super) sim: TickState,
    /// On-disk save handle (`None` if saving is disabled / failed to open).
    pub(super) save: Option<WorldSave>,
    /// Active dropped item entities — those resting in currently-loaded chunks.
    /// Items unload with their chunk (serialized into its save record) and reload
    /// with it, so this collection only ever holds drops the player can actually
    /// see. The entity subsystem (physics, pickup, lifetime, save-bundling) lives
    /// on [`DroppedItems`]; see `world::entities`.
    pub(super) dropped_items: DroppedItems,
    /// Active mobs — those in currently-loaded chunks. Like [`dropped_items`], a mob
    /// unloads with its chunk (saved into its record) and reloads with it, so this only
    /// holds mobs the player can see. AI/physics/spawning + the save-bundling live on
    /// [`Mobs`]; `Game` drives them through the world (the `tick_mobs` /
    /// `spawn_mobs_tick` borrow-splits) and reads back the loot a kill drops. See
    /// `mob::manager`.
    pub(super) mobs: Mobs,
}

impl World {
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self {
            seed,
            chunks: HashMap::new(),
            meshes: HashMap::new(),
            worker: WorkerPool::new(seed),
            pending: HashMap::new(),
            render_dist,
            section_visibility: HashMap::new(),
            visibility_revision: 0,
            lighting_revision: 0,
            light_bakes: LightBakeQueue::new(),
            dirty_meshes: DirtyMeshQueue::default(),
            last_load_target: None,
            sim: TickState::new(seed),
            save: None,
            dropped_items: DroppedItems::default(),
            mobs: Mobs::new(seed as u64),
        }
    }

    /// Attach an on-disk save: enables chunk persistence (load-from-disk in the
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
    /// (autosave/quit) and `unload_far_chunks` (eviction). Applies the three-way
    /// persist condition and, when it holds, builds the chunk's [`ChunkSnapshot`]
    /// with `entities` attached; returns `None` when the chunk needn't persist.
    ///
    /// The gate persists a chunk when ANY of:
    /// - its blocks were modified,
    /// - it carries item entities or mobs right now, or
    /// - `record_holds_entities` — its on-disk record still holds drops/mobs it no
    ///   longer carries, so the stale record must be rewritten or it resurrects them on
    ///   reload. This signal is CROSS-SESSION: the caller computes it from the save
    ///   handle (`WorldSave::record_holds_entities`), which also remembers records read
    ///   back from a prior session (`note_record_holds_entities`), so a record written
    ///   before this run is still cleared once its entities are gone.
    ///
    /// The caller owns the harvest policy (which fed `entities` / `mobs`) and the
    /// post-action (clear `modified` vs. evict), keeping flush's "stay active" and
    /// unload's "pause / save" lifetimes distinct.
    ///
    /// [`flush_modified_chunks`]: Self::flush_modified_chunks
    /// [`ChunkSnapshot`]: crate::save::ChunkSnapshot
    pub(super) fn snapshot_chunk_for_save(
        &self,
        pos: ChunkPos,
        entities: Vec<DroppedItem>,
        mobs: Vec<SavedMob>,
        record_holds_entities: bool,
    ) -> Option<ChunkSnapshot> {
        let chunk = self.chunks.get(&pos)?;
        if chunk.modified || !entities.is_empty() || !mobs.is_empty() || record_holds_entities {
            let mut snap = ChunkSnapshot::from_chunk(chunk);
            snap.entities = entities;
            snap.mobs = mobs;
            Some(snap)
        } else {
            None
        }
    }

    /// Snapshot every modified chunk to the save thread and clear the flags.
    /// Also snapshots any chunk holding item entities (even if its blocks are
    /// untouched) so their lifetime timers persist; the entities stay active in
    /// memory. Called on autosave and on quit; a no-op without an attached save.
    pub fn flush_modified_chunks(&mut self) {
        if self.save.is_none() {
            return;
        }
        // Flush's harvest policy: CLONE the resting drops and mobs (they stay active in
        // memory) so a crash can't lose them.
        let mut by_chunk = self.dropped_items.items_by_chunk();
        let mut mobs_by_chunk = self.mobs.saved_by_chunk();
        let positions: Vec<ChunkPos> = self.chunks.keys().copied().collect();
        let mut snaps = Vec::new();
        let mut persisted = Vec::new();
        for pos in positions {
            let entities = by_chunk.remove(&pos).unwrap_or_default();
            let mobs = mobs_by_chunk.remove(&pos).unwrap_or_default();
            let record_holds_entities = self
                .save
                .as_ref()
                .is_some_and(|s| s.record_holds_entities(pos));
            if let Some(snap) =
                self.snapshot_chunk_for_save(pos, entities, mobs, record_holds_entities)
            {
                snaps.push(snap);
                persisted.push(pos);
            }
        }
        // Post-action: a persisted chunk is now in sync with disk.
        for pos in persisted {
            if let Some(c) = self.chunks.get_mut(&pos) {
                c.modified = false;
            }
        }
        if let Some(save) = self.save.as_mut() {
            save.save_chunks(snaps);
        }
    }

    /// The active mobs (read-only), for `Game` to forward to the render-side scene
    /// adapter and to ray-test for crosshair targeting.
    #[inline]
    pub fn mobs(&self) -> &Mobs {
        &self.mobs
    }

    /// Mutable access to the active mobs, so `Game` can spawn (the debug owl key), apply
    /// an attack (reading back the loot a kill drops), and otherwise drive them — while
    /// the live set stays owned here, persisting with the chunks the mobs stand in.
    #[inline]
    pub fn mobs_mut(&mut self) -> &mut Mobs {
        &mut self.mobs
    }

    /// Advance the mobs one fixed game tick (AI, physics, soft entity pushing, hostile
    /// distance-despawn). Drives the owned [`Mobs`] against an immutable view of the rest
    /// of the world: the field is moved out so the `&mut Mobs` and `&World` borrows stay
    /// disjoint, mirroring [`tick_item_physics`](Self::tick_item_physics). With a save
    /// attached, a mob over a not-yet-loaded chunk is frozen so it can't fall through
    /// missing terrain.
    ///
    /// `player_pos` is the player's body centre (the AI's player anchor). `player_body`
    /// is the player's *pushable* body — present only when the player has a physical
    /// presence (`None` for a noclip spectator) — so the mobs are shoved off it
    /// (player→mob) on the tick. The reverse push on the *player* is applied per-frame by
    /// the caller (it moves the player, which integrates per-frame); see
    /// [`Mobs::push_on_player`].
    pub fn tick_mobs(&mut self, dt: f32, player_pos: Vec3, player_body: Option<crate::mob::Body>) {
        if self.mobs.is_empty() {
            return;
        }
        let freeze_unloaded = self.save.is_some();
        let mut mobs = std::mem::take(&mut self.mobs);
        mobs.tick(dt, self, player_pos, player_body, freeze_unloaded);
        self.mobs = mobs;
    }

    /// Run one natural mob-spawn attempt (one per game tick). Same borrow-split as
    /// [`tick_mobs`](Self::tick_mobs); never early-returns, since an empty set is exactly
    /// when a spawn may add the first mob.
    pub fn spawn_mobs_tick(&mut self, player_pos: Vec3) {
        let mut mobs = std::mem::take(&mut self.mobs);
        mobs.spawn_tick(self, player_pos);
        self.mobs = mobs;
    }

    #[inline]
    pub fn lighting_revision(&self) -> u64 {
        self.lighting_revision
    }

    pub(super) fn bump_lighting_revision(&mut self) {
        self.lighting_revision = self.lighting_revision.wrapping_add(1);
    }

    pub(super) fn mark_dirty_pos(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.chunks.get_mut(&pos) {
            chunk.dirty = true;
            self.dirty_meshes.push(pos);
        }
    }

    pub(super) fn mark_light_dirty_pos(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.chunks.get_mut(&pos) {
            chunk.mark_light_dirty();
        }
    }

    pub(super) fn queue_dirty_mesh(&mut self, pos: ChunkPos) {
        if self.chunks.contains_key(&pos) {
            self.dirty_meshes.push(pos);
        }
    }

    /// The one world-coordinate router: decode a world voxel `(wx, wy, wz)` into
    /// its owning chunk and in-chunk local coords `(ChunkPos, lx, ly, lz)`, or
    /// `None` when `wy` falls outside the `0..CHUNK_SY` column. Chunk lookup is a
    /// separate step (see [`chunk_at_world`](Self::chunk_at_world)): callers that
    /// only need the decode (e.g. to address an unloaded chunk) stop here. The
    /// out-of-range *fallback value* stays with each caller — this returns `None`.
    #[inline]
    pub(super) fn split_world(wx: i32, wy: i32, wz: i32) -> Option<(ChunkPos, usize, usize, usize)> {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return None;
        }
        Some((
            ChunkPos::new(wx >> 4, wz >> 4),
            chunk::lx(wx),
            wy as usize,
            chunk::lz(wz),
        ))
    }

    /// The loaded chunk owning world voxel `(wx, wy, wz)` plus its local coords,
    /// or `None` if `wy` is out of range or the chunk is not loaded. The shared
    /// front end for every read-side world-coordinate accessor.
    #[inline]
    pub(super) fn chunk_at_world(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(&Chunk, usize, usize, usize)> {
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let c = self.chunks.get(&pos)?;
        Some((c, lx, ly, lz))
    }

    /// Mutable counterpart of [`chunk_at_world`](Self::chunk_at_world): the loaded
    /// owning chunk and local coords for a write-side accessor, or `None` when out
    /// of range or unloaded.
    #[inline]
    pub(super) fn chunk_at_world_mut(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<(&mut Chunk, usize, usize, usize)> {
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let c = self.chunks.get_mut(&pos)?;
        Some((c, lx, ly, lz))
    }

    /// Mark the chunk owning world voxel `pos` as modified, so a change that no
    /// tick would otherwise re-flag (a GUI edit to an idle chest or furnace)
    /// still persists. No-op if the chunk is not loaded.
    pub fn mark_chunk_modified(&mut self, pos: crate::mathh::IVec3) {
        if let Some(c) = self.chunks.get_mut(&ChunkPos::new(pos.x >> 4, pos.z >> 4)) {
            c.modified = true;
        }
    }

    pub(super) fn mark_dirty_neighborhood(&mut self, center: ChunkPos, include_center: bool) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if !include_center && dx == 0 && dz == 0 {
                    continue;
                }
                self.mark_dirty_pos(ChunkPos::new(center.cx + dx, center.cz + dz));
            }
        }
    }

    pub(super) fn mark_light_dirty_neighborhood(&mut self, center: ChunkPos, include_center: bool) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                if !include_center && dx == 0 && dz == 0 {
                    continue;
                }
                self.mark_light_dirty_pos(ChunkPos::new(center.cx + dx, center.cz + dz));
            }
        }
    }

    pub(super) fn remove_chunk(&mut self, pos: ChunkPos) {
        self.chunks.remove(&pos);
        self.meshes.remove(&pos);
        self.pending.remove(&pos);
        self.dirty_meshes.remove(pos);
        self.light_bakes.cancel(pos);
        self.mark_light_dirty_neighborhood(pos, false);
        self.mark_dirty_neighborhood(pos, false);
        if self.section_visibility.remove(&pos).is_some() {
            self.bump_visibility_revision();
        }
    }

    /// Drop all loaded chunks, their meshes, and the in-flight gen set — the
    /// regen path. Invalidates the section-visibility cache so the renderer
    /// rebuilds from scratch on the next frame.
    pub fn clear_world(&mut self) {
        self.chunks.clear();
        self.meshes.clear();
        self.pending.clear();
        if !self.section_visibility.is_empty() {
            self.section_visibility.clear();
            self.bump_visibility_revision();
        }
    }

    /// Install a chunk for a test, bypassing generation but mirroring the
    /// streamer's per-chunk install (`stream::poll`): drop it in, invalidate the
    /// section-visibility cache, and enqueue it for meshing. So a test chunk
    /// enters the dirty-mesh queue exactly as a streamed one would.
    /// Test-only: production loads chunks through the streamer.
    #[cfg(test)]
    pub(crate) fn insert_chunk_for_test(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.chunks.insert(pos, chunk);
        self.invalidate_section_visibility(pos);
        self.queue_dirty_mesh(pos);
    }

    /// Mutable access to an installed chunk for a test that pokes voxel state
    /// directly (e.g. seeding water metadata). Test-only: production edits go
    /// through `set_block_world` so the dirty-mesh queue stays authoritative.
    #[cfg(test)]
    pub(crate) fn chunk_mut_for_test(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_rewrites_a_chunk_whose_record_holds_a_picked_up_drop() {
        // The quit/reopen variant of the dupe: the autosave/quit flush must also
        // rewrite a chunk whose record holds a drop the chunk no longer carries.
        let dir = std::env::temp_dir().join(format!(
            "llamacraft-flushtest-{}-rewrite",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut opened = crate::save::open_at(dir.clone()).expect("open temp world");

        let pos = ChunkPos::new(0, 0);
        opened.save.note_record_holds_entities(pos); // as the load path would
        let mut world = World::new(0, 1);
        world.attach_save(opened.save);
        world.chunks.insert(pos, Chunk::new(pos.cx, pos.cz));

        world.flush_modified_chunks();

        assert!(
            !world.save().expect("save").record_holds_entities(pos),
            "flush must rewrite the chunk and clear its stale drop record"
        );

        drop(world); // join the save I/O thread before removing the dir
        let _ = std::fs::remove_dir_all(&dir);
    }
}
