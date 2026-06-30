use crate::chunk::{ChunkPos, SectionPos};
use crate::mesh::ChunkMesh;

use super::World;

pub(crate) struct TerrainRenderHandoff<'a> {
    world: &'a mut World,
}

impl World {
    pub(crate) fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        TerrainRenderHandoff { world: self }
    }
}

impl TerrainRenderHandoff<'_> {
    pub(crate) fn has_column_mesh(&self, pos: ChunkPos) -> bool {
        self.world.column_has_mesh(pos)
    }

    pub(crate) fn for_dirty_columns(&self, f: &mut dyn FnMut(ChunkPos)) {
        for &column in &self.world.mesh_upload_dirty_columns {
            f(column);
        }
    }

    pub(crate) fn column_meshes(&self, pos: ChunkPos) -> Vec<(SectionPos, &ChunkMesh)> {
        World::column_section_range()
            .filter_map(|cy| {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                self.world.meshes.get(&sp).map(|mesh| (sp, mesh))
            })
            .collect()
    }

    pub(crate) fn mark_column_uploaded(&mut self, pos: ChunkPos) {
        for cy in World::column_section_range() {
            if let Some(mesh) = self
                .world
                .meshes
                .get_mut(&SectionPos::new(pos.cx, cy, pos.cz))
            {
                mesh.mesh_dirty = false;
            }
        }
        self.world.mesh_upload_dirty_columns.remove(&pos);
    }
}
