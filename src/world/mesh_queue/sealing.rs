use crate::chunk::SectionPos;
use crate::world::store::World;

impl World {
    /// Whether `pos` produces no visible geometry, so meshing/lighting/drawing it is pure
    /// waste: it is entirely air (the empty-sky band) and emits nothing. This is the exact
    /// counter-based case ONLY. The neighbour-plane "sealed section" skip that used to
    /// live here was removed on 2026-07-06 after playtests traced black (unlit) faces to
    /// section culling — do not reintroduce it here.
    pub(in crate::world) fn section_produces_no_mesh(&self, pos: SectionPos) -> bool {
        self.sections.get(&pos).is_some_and(|s| s.is_empty_air())
    }

    /// Exact future-work skip: every adjoining plane is a loaded, fully opaque
    /// wall, so no outside sightline or emitted boundary face can reach this
    /// section. A nearby player overrides the proof because they may already be
    /// inside an enclosed cave. Generated summaries are deliberately not trusted
    /// for saved terrain.
    pub(in crate::world) fn section_sealed_by_loaded_neighbors(&self, pos: SectionPos) -> bool {
        if self.last_load_target.is_some() && self.near_load_center(pos) {
            return false;
        }
        crate::mathh::FACE_NEIGHBORS.into_iter().all(|d| {
            self.sections
                .get(&SectionPos::new(pos.cx + d.x, pos.cy + d.y, pos.cz + d.z))
                .is_some_and(|s| s.face_plane_fully_opaque(-d.x, -d.y, -d.z))
        })
    }

    /// Clear stale render output for a section that now intentionally emits no mesh.
    /// Returns true when the section is in that settled no-output state.
    pub(in crate::world) fn clear_mesh_if_section_produces_no_mesh(
        &mut self,
        pos: SectionPos,
    ) -> bool {
        if !self.section_produces_no_mesh(pos) {
            return false;
        }
        if self.remove_mesh(pos) {
            self.mesh_upload_dirty_columns.insert(pos.chunk_pos());
        }
        self.dirty_meshes.remove(pos);
        self.light_blocked_meshes.remove(&pos);
        self.hidden_parked.remove(&pos);
        self.sealed_parked.remove(&pos);
        if let Some(s) = self.section_mut(pos) {
            s.dirty = false;
            // A mesh job may already have snapshotted this section while one of its
            // now-solid neighbours was still missing. Invalidate that exposed-border
            // result so it cannot reinstall geometry after we settle to no output.
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
        }
        true
    }
}
