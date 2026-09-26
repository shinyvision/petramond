use crate::world::ReplicaWorld;
use petramond_mesh::ChunkMesh;
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::super::side::TerrainRenderState;
use super::column_cy_bit;

/// The replica's mesh store and the per-column indexes derived from it — the
/// single funnel that keeps `meshes`, `mesh_columns`, `mesh_column_cys` and
/// the upload bookkeeping in step.
impl TerrainRenderState {
    #[inline]
    pub(in crate::world) fn column_has_mesh(&self, pos: ChunkPos) -> bool {
        self.mesh_columns.contains(&pos)
    }

    pub(in crate::world) fn install_mesh(&mut self, pos: SectionPos, mesh: ChunkMesh) {
        self.meshes.insert(pos, mesh);
        self.repack_forced.remove(&pos);
        let column = pos.chunk_pos();
        self.mesh_columns.insert(column);
        *self.mesh_column_cys.entry(column).or_insert(0) |= column_cy_bit(pos.cy);
        self.bump_mesh_upload_revision(column);
        self.mesh_upload_dirty_columns.insert(column);
    }

    pub(in crate::world) fn remove_mesh(&mut self, pos: SectionPos) -> bool {
        let removed = self.meshes.remove(&pos).is_some();
        self.repack_forced.remove(&pos);
        if removed {
            let column = pos.chunk_pos();
            self.clear_mesh_cy(column, pos.cy);
            self.bump_mesh_upload_revision(column);
        }
        removed
    }

    fn bump_mesh_upload_revision(&mut self, pos: ChunkPos) {
        let revision = self.mesh_upload_revisions.entry(pos).or_insert(0);
        *revision = revision.wrapping_add(1).max(1);
    }

    fn clear_mesh_cy(&mut self, pos: ChunkPos, cy: i32) {
        let Some(bits) = self.mesh_column_cys.get_mut(&pos) else {
            self.mesh_columns.remove(&pos);
            return;
        };
        *bits &= !column_cy_bit(cy);
        if *bits == 0 {
            self.mesh_column_cys.remove(&pos);
            self.mesh_columns.remove(&pos);
        }
    }
}

impl ReplicaWorld {
    /// Iterate loaded section meshes for rendering (caller culls by camera).
    pub fn iter_meshes(&self) -> impl Iterator<Item = (SectionPos, &ChunkMesh)> {
        self.side.terrain.meshes.iter().map(|(p, m)| (*p, m))
    }
}
