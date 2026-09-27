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
    pub trait Sealed {}
}

pub trait WorldSide: sealed::Sealed + Sized + 'static {
    fn server(&self) -> Option<&ServerSide> {
        None
    }

    fn server_mut(&mut self) -> Option<&mut ServerSide> {
        None
    }

    fn replica(&self) -> Option<&ReplicaSide> {
        None
    }

    fn replica_mut(&mut self) -> Option<&mut ReplicaSide> {
        None
    }
}

pub struct ServerSide {
    pub(in crate::world) gen: WorldgenJobs,
    pub(in crate::world) replication: ReplicationLog,
    pub(in crate::world) session: SessionState,
    pub(in crate::world) worker: WorkerPool,
    pub(in crate::world) save: Option<WorldSave>,
    pub(in crate::world) entities: EntityStores,
    pub(in crate::world) schematics: super::schematic::WorldSchematics,
    pub(in crate::world) stream_events: Vec<StreamEvent>,
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

pub struct ReplicaSide {
    pub(in crate::world) terrain: TerrainRenderState,
    pub(in crate::world) changes: super::remote::Changes,
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

pub(in crate::world) struct TerrainRenderState {
    pub(in crate::world) meshes: FxHashMap<SectionPos, ChunkMesh>,
    pub(in crate::world) mesh_columns: FxHashSet<ChunkPos>,
    pub(in crate::world) mesh_column_cys: FxHashMap<ChunkPos, u32>,
    pub(in crate::world) mesh_upload_revisions: FxHashMap<ChunkPos, u64>,
    pub(in crate::world) mesh_upload_dirty_columns: FxHashSet<ChunkPos>,
    pub(in crate::world) upload_urgent_columns: FxHashSet<ChunkPos>,
    pub(in crate::world) mesh_release_after: FxHashMap<ChunkPos, u64>,
    pub(in crate::world) repack_forced: FxHashSet<SectionPos>,
    pub(in crate::world) mesh_pump_frame: u64,
    pub(in crate::world) mesh_pump_now: std::time::Instant,
    pub(in crate::world) mesh_settle: FxHashMap<SectionPos, super::mesh_queue::settle::MeshSettle>,
    pub(in crate::world) mesh_pool: super::mesh_pool::MeshPool,
    pub(in crate::world) mesh_jobs_in_flight: usize,
    pub(in crate::world) mesh_job_cancels: FxHashMap<SectionPos, JobCancel>,
    pub(in crate::world) dirty_meshes: DirtyMeshQueue,
    pub(in crate::world) deep_sections: FxHashSet<SectionPos>,
    pub(in crate::world) visible_deep: FxHashSet<SectionPos>,
    pub(in crate::world) hidden_parked: FxHashSet<SectionPos>,
    pub(in crate::world) sealed_parked: FxHashSet<SectionPos>,
    pub(in crate::world) prediction_terrain: super::prediction_render::PredictionTerrainQueue,
    pub(in crate::world) vis_dirty: bool,
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
            mesh_pump_now: std::time::Instant::now(),
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

pub(in crate::world) struct WorldgenJobs {
    pub(in crate::world) column_gen: FxHashMap<ChunkPos, Arc<ColumnGen>>,
    pub(in crate::world) pending: FxHashMap<ChunkPos, Option<GenJobHandle>>,
    pub(in crate::world) pending_sections: FxHashSet<SectionPos>,
    pub(in crate::world) pending_section_columns: FxHashMap<ChunkPos, u16>,
    pub(in crate::world) pending_section_jobs: FxHashMap<SectionPos, GenJobHandle>,
    pub(in crate::world) section_requests_unsettled: bool,
    pub(in crate::world) pending_overlays: FxHashMap<SectionPos, super::stream::LoadedOverlay>,
    pub(in crate::world) awaited_overlays: FxHashSet<SectionPos>,
    pub(in crate::world) disk_primary_sections: FxHashSet<SectionPos>,
    pub(in crate::world) pending_colgen_records: Vec<crate::save::colgen::ColumnGenRecord>,
    pub(in crate::world) populated_columns: BTreeSet<ChunkPos>,
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

#[derive(Default)]
pub(in crate::world) struct ReplicationLog {
    pub(in crate::world) replication_capture: bool,
    pub(in crate::world) block_delta_log: FxHashMap<IVec3, BlockDelta>,
    pub(in crate::world) cell_kv_delta_log: FxHashMap<(IVec3, String), Option<Vec<u8>>>,
    pub(in crate::world) block_draw_log: FxHashSet<IVec3>,
    pub(in crate::world) light_ship_log: FxHashSet<SectionPos>,
    pub(in crate::world) terrain_revision: u64,
}

impl ReplicationLog {
    #[inline]
    pub(in crate::world) fn bump_terrain_revision(&mut self) {
        self.terrain_revision = self.terrain_revision.wrapping_add(1);
    }

    #[inline]
    pub(in crate::world) fn wants_cell_kv_delta(&self, pos: IVec3) -> bool {
        self.replication_capture && !self.block_delta_log.contains_key(&pos)
    }
}

pub(in crate::world) struct SessionState {
    pub(in crate::world) player_inputs: Vec<super::session::PlayerInputSnapshot>,
    pub(in crate::world) player_roster: Vec<super::session::PlayerRosterSnapshot>,
    pub(in crate::world) keep_inventory: bool,
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

pub(in crate::world) struct EntityStores {
    pub(in crate::world) dropped_items: DroppedItems,
    pub(in crate::world) mobs: Mobs,
    pub(in crate::world) riding: crate::mob::riding::Riding,
    pub(in crate::world) nav_probe_budget: crate::mob::ReachBudget,
    pub(in crate::world) route_probe_budget: crate::mob::ReachBudget,
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
