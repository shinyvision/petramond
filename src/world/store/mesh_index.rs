use crate::chunk::{ChunkPos, SectionPos};
use crate::mesh::ChunkMesh;

use super::World;

impl World {
    #[inline]
    pub(in crate::world) fn column_has_mesh(&self, pos: ChunkPos) -> bool {
        self.mesh_columns.contains(&pos)
    }

    /// Iterate the meshed section `cy` values recorded for a column bitset.
    #[inline]
    pub(in crate::world) fn for_each_mesh_cy(bits: u32, f: impl FnMut(i32)) {
        Self::for_each_column_cy(bits, f);
    }

    pub(in crate::world) fn install_mesh(&mut self, pos: SectionPos, mesh: ChunkMesh) {
        self.meshes.insert(pos, mesh);
        self.repack_forced.remove(&pos);
        let column = pos.chunk_pos();
        self.mesh_columns.insert(column);
        *self.mesh_column_cys.entry(column).or_insert(0) |= Self::column_cy_bit(pos.cy);
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
        *bits &= !Self::column_cy_bit(cy);
        if *bits == 0 {
            self.mesh_column_cys.remove(&pos);
            self.mesh_columns.remove(&pos);
        }
    }

    /// Drop the column's mesh-index entries (meshes themselves already removed).
    pub(in crate::world) fn clear_mesh_column_index(&mut self, pos: ChunkPos) {
        self.mesh_columns.remove(&pos);
        self.mesh_column_cys.remove(&pos);
    }
}
