use crate::chunk::ChunkPos;
use crate::mesh::ChunkMesh;

use super::visibility::{SectionConnectivity, SectionPos};
use super::World;

pub(crate) struct TerrainRenderHandoff<'a> {
    world: &'a mut World,
}

pub(crate) trait TerrainMeshUploadSource {
    fn has_mesh(&self, pos: ChunkPos) -> bool;

    fn for_each_mesh_upload<F>(&mut self, visit: F)
    where
        F: for<'mesh> FnMut(ChunkPos, &'mesh ChunkMesh, bool) -> bool;
}

pub(crate) trait TerrainVisibilitySource {
    fn visibility_revision(&self) -> u64;
    fn camera_section_exits(&self, wx: i32, wy: i32, wz: i32) -> Option<(SectionPos, u8)>;
    fn has_section_visibility(&self, pos: ChunkPos) -> bool;
    fn chunk_loaded(&self, pos: ChunkPos) -> bool;
    fn ensure_section_visibility(&mut self, pos: ChunkPos) -> bool;
    fn section_connectivity(&self, pos: SectionPos) -> Option<SectionConnectivity>;
}

impl World {
    pub(crate) fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        TerrainRenderHandoff { world: self }
    }
}

impl TerrainMeshUploadSource for TerrainRenderHandoff<'_> {
    fn has_mesh(&self, pos: ChunkPos) -> bool {
        self.world.meshes.contains_key(&pos)
    }

    fn for_each_mesh_upload<F>(&mut self, mut visit: F)
    where
        F: for<'mesh> FnMut(ChunkPos, &'mesh ChunkMesh, bool) -> bool,
    {
        for (pos, mesh) in self.world.meshes.iter_mut() {
            let uploaded = visit(*pos, mesh, mesh.mesh_dirty);
            if uploaded {
                mesh.mesh_dirty = false;
            }
        }
    }
}

impl TerrainVisibilitySource for TerrainRenderHandoff<'_> {
    fn visibility_revision(&self) -> u64 {
        self.world.visibility_revision
    }

    fn camera_section_exits(&self, wx: i32, wy: i32, wz: i32) -> Option<(SectionPos, u8)> {
        self.world.camera_section_exits(wx, wy, wz)
    }

    fn has_section_visibility(&self, pos: ChunkPos) -> bool {
        self.world.has_section_visibility(pos)
    }

    fn chunk_loaded(&self, pos: ChunkPos) -> bool {
        self.world.chunk_loaded(pos.cx, pos.cz)
    }

    fn ensure_section_visibility(&mut self, pos: ChunkPos) -> bool {
        self.world.ensure_section_visibility(pos)
    }

    fn section_connectivity(&self, pos: SectionPos) -> Option<SectionConnectivity> {
        self.world.section_connectivity(pos)
    }
}
