use crate::world::{ReplicaWorld, World, WorldSide};
use petramond_world::chunk::SectionPos;

impl<S: WorldSide> World<S> {
    pub(in crate::world) fn section_produces_no_mesh(&self, pos: SectionPos) -> bool {
        self.data
            .sections
            .get(&pos)
            .is_some_and(|s| s.is_empty_air())
    }

    pub(in crate::world) fn section_sealed_by_loaded_neighbors(&self, pos: SectionPos) -> bool {
        if self.data.last_load_target.is_some() && self.near_load_center(pos) {
            return false;
        }
        petramond_math::math::FACE_NEIGHBORS.into_iter().all(|d| {
            self.data
                .sections
                .get(&SectionPos::new(pos.cx + d.x, pos.cy + d.y, pos.cz + d.z))
                .is_some_and(|s| s.face_plane_fully_opaque(-d.x, -d.y, -d.z))
        })
    }
}

impl ReplicaWorld {
    pub(in crate::world) fn clear_mesh_if_section_produces_no_mesh(
        &mut self,
        pos: SectionPos,
    ) -> bool {
        if !self.section_produces_no_mesh(pos) {
            return false;
        }
        if self.side.terrain.remove_mesh(pos) {
            self.side
                .terrain
                .mesh_upload_dirty_columns
                .insert(pos.chunk_pos());
        }
        self.side.terrain.dirty_meshes.remove(pos);
        self.side.terrain.light_blocked_meshes.remove(&pos);
        self.side.terrain.hidden_parked.remove(&pos);
        self.side.terrain.sealed_parked.remove(&pos);
        if let Some(s) = self.data.section_mut(pos) {
            s.dirty = false;
            // A mesh job may already have snapshotted this section while one of its
            // now-solid neighbours was still missing. Invalidate that exposed-border
            // result so it cannot reinstall geometry after we settle to no output.
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
        }
        true
    }
}
