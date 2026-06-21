use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkPos, SECTION_COUNT};
use crate::mesh::ChunkMesh;
use crate::worker::WorkerPool;

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
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub meshes: HashMap<ChunkPos, ChunkMesh>,
    pub worker: WorkerPool,
    /// Chunks queued for gen (waiting on result).
    pub pending: HashMap<ChunkPos, ()>,
    pub render_dist: i32,
    pub section_visibility: HashMap<ChunkPos, [SectionConnectivity; SECTION_COUNT]>,
    pub visibility_revision: u64,
    pub(super) lighting_revision: u64,
    pub(super) light_bakes: LightBakeQueue,
    pub(super) dirty_meshes: DirtyMeshQueue,
    pub(super) last_load_target: Option<LoadTarget>,
    /// Fixed-timestep simulation state: block updates + scheduled block ticks.
    pub(super) sim: TickState,
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
            sim: TickState::default(),
        }
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
}
