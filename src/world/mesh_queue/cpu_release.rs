use crate::world::store::for_each_column_cy;
use crate::world::ReplicaWorld;
use petramond_world::chunk::{ChunkPos, SectionPos};

use super::{MESH_RELEASE_DELAY_FRAMES, MESH_RELEASE_SWEEP_INTERVAL};

impl ReplicaWorld {
    pub(super) fn release_settled_column_meshes(&mut self) {
        if !self
            .side
            .terrain
            .mesh_pump_frame
            .is_multiple_of(MESH_RELEASE_SWEEP_INTERVAL)
            || self.side.terrain.mesh_release_after.is_empty()
        {
            return;
        }
        let frame = self.side.terrain.mesh_pump_frame;
        let ripe: Vec<ChunkPos> = self
            .side
            .terrain
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
                self.side
                    .terrain
                    .mesh_release_after
                    .insert(pos, frame + MESH_RELEASE_DELAY_FRAMES);
                continue;
            }
            self.side.terrain.mesh_release_after.remove(&pos);
            if self.side.terrain.mesh_upload_dirty_columns.contains(&pos) {
                continue;
            }
            let Some(&bits) = self.side.terrain.mesh_column_cys.get(&pos) else {
                continue;
            };
            let mut busy = false;
            for_each_column_cy(bits, |cy| {
                let sp = SectionPos::new(pos.cx, cy, pos.cz);
                if self.side.terrain.dirty_meshes.contains(sp)
                    || self.side.terrain.light_blocked_meshes.contains(&sp)
                {
                    busy = true;
                }
            });
            if busy {
                continue;
            }
            for_each_column_cy(bits, |cy| {
                if let Some(mesh) = self
                    .side
                    .terrain
                    .meshes
                    .get_mut(&SectionPos::new(pos.cx, cy, pos.cz))
                {
                    if !mesh.mesh_dirty && !mesh.is_released() {
                        mesh.release_cpu_buffers();
                    }
                }
            });
        }
    }
}
