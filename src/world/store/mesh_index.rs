use crate::chunk::{ChunkPos, SectionPos};
use crate::mesh::ChunkMesh;

use super::World;

impl World {
    #[inline]
    pub(in crate::world) fn column_has_mesh(&self, pos: ChunkPos) -> bool {
        self.mesh_columns.contains(&pos)
    }

    pub(in crate::world) fn install_mesh(&mut self, pos: SectionPos, mesh: ChunkMesh) {
        self.meshes.insert(pos, mesh);
        self.repack_forced.remove(&pos);
        let column = pos.chunk_pos();
        self.mesh_columns.insert(column);
        self.bump_mesh_upload_revision(column);
        self.mesh_upload_dirty_columns.insert(column);
    }

    pub(in crate::world) fn remove_mesh(&mut self, pos: SectionPos) -> bool {
        let removed = self.meshes.remove(&pos).is_some();
        self.repack_forced.remove(&pos);
        if removed {
            let column = pos.chunk_pos();
            self.refresh_mesh_column_presence(column);
            self.bump_mesh_upload_revision(column);
        }
        removed
    }

    fn bump_mesh_upload_revision(&mut self, pos: ChunkPos) {
        let revision = self.mesh_upload_revisions.entry(pos).or_insert(0);
        *revision = revision.wrapping_add(1).max(1);
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
}
