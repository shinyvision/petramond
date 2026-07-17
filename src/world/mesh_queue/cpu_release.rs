use crate::chunk::{ChunkPos, SectionPos};
use crate::world::store::World;

use super::{MESH_RELEASE_DELAY_FRAMES, MESH_RELEASE_SWEEP_INTERVAL};

impl World {
    /// Release the CPU mesh buffers of columns that have been upload-quiet for
    /// [`MESH_RELEASE_DELAY_FRAMES`] (stamped by `mark_column_uploaded`). The CPU
    /// copy only exists so a column repack can re-pack sibling sections; once a
    /// column settles, the copy is dead weight (~30–60 KB per meshed section) and
    /// a later repack forces a remesh of the released sections instead
    /// (`repack_forced`). Releasing never touches the GPU buffers, so a wrong
    /// "settled" verdict costs remesh work, never visible terrain.
    pub(super) fn release_settled_column_meshes(&mut self) {
        if !self
            .mesh_pump_frame
            .is_multiple_of(MESH_RELEASE_SWEEP_INTERVAL)
            || self.mesh_release_after.is_empty()
        {
            return;
        }
        let frame = self.mesh_pump_frame;
        let ripe: Vec<ChunkPos> = self
            .mesh_release_after
            .iter()
            .filter(|&(_, &after)| frame >= after)
            .map(|(&pos, _)| pos)
            .collect();
        for pos in ripe {
            // Keep the columns around every load anchor resident: the player
            // edits there, and an edit into a released column forces a remesh
            // of every released sibling before the packed upload can happen —
            // a whole-column remesh storm on the first click after idling.
            // Bounded cost: (2·ring+1)² columns per anchor stay at full size;
            // the re-armed timer releases them once the anchor moves away.
            if self.column_near_load_center(pos) {
                self.mesh_release_after
                    .insert(pos, frame + MESH_RELEASE_DELAY_FRAMES);
                continue;
            }
            self.mesh_release_after.remove(&pos);
            // Still has upload or remesh work pending: skip. The eventual upload
            // re-stamps the column via `mark_column_uploaded`.
            if self.mesh_upload_dirty_columns.contains(&pos) {
                continue;
            }
            let busy = Self::column_section_range().any(|cy| {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                self.dirty_meshes.contains(sp) || self.light_blocked_meshes.contains(&sp)
            });
            if busy {
                continue;
            }
            for cy in Self::column_section_range() {
                if let Some(mesh) = self.meshes.get_mut(&SectionPos::new(pos.cx, cy, pos.cz)) {
                    if !mesh.mesh_dirty && !mesh.is_released() {
                        mesh.release_cpu_buffers();
                    }
                }
            }
        }
    }
}
