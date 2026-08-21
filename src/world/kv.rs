//! Persistent mod data on the world: the world KV
//! map (rides `level.dat`, restored at session open) and the per-cell section
//! KV accessors (each cell's entries ride its section's save record).
//!
//! Namespacing (`mod_id:key`, own-prefix writes) is enforced at the HostCall
//! boundary (`modding::host`), not here — engine/test code may use any key.
//! The GUI-session state map is NOT here: it lives on the player session
//! (`ConnectedPlayer::gui_state` + the `crate::gui` state helpers).

use super::store::World;

impl World {
    /// Store a cell KV entry; marks the section modified so the data persists.
    /// `false` = the owning section is unloaded / out of range / not
    /// stream-final (a write racing an in-flight saved overlay would be
    /// clobbered when it lands — refuse, like `set_block_world`).
    pub fn cell_kv_set(&mut self, wx: i32, wy: i32, wz: i32, key: String, value: Vec<u8>) -> bool {
        if !self.cell_kv_writable(wx, wy, wz) {
            return false;
        }
        let pos = petramond_math::math::IVec3::new(wx, wy, wz);
        // Capture only when NO block delta covers this cell this tick: that
        // delta's drain re-reads the cell's whole KV map, so a separate KV
        // delta would be redundant — and one logged BEFORE a wiping block
        // write would be stale (`record_block_delta` scrubs those).
        let capture = self.replication.replication_capture
            && !self.replication.block_delta_log.contains_key(&pos);
        let log_value = capture.then(|| value.clone());
        let Some((s, lx, ly, lz)) = self.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        s.cell_kv_set(lx, ly, lz, key.clone(), value);
        s.modified = true;
        if let Some(v) = log_value {
            self.replication
                .cell_kv_delta_log
                .insert((pos, key.clone()), Some(v));
        }
        // A mesh-feeding presentation write renders through the mesher —
        // re-mesh the host's own view (the replica side re-meshes in its KV
        // ingest).
        if petramond_world::block::kv_key_affects_mesh(&key) {
            self.queue_dirty_meshes_sampling_cell(wx, wy, wz);
        }
        // A stateful custom shape resolving from this key (here or a
        // neighbour) must re-bake — its geometry and collision derive from it.
        self.remark_state_key_bakes(wx, wy, wz, &key);
        true
    }
    /// Remove a cell KV entry; returns whether it was present. A removal marks
    /// the section modified so the stale on-disk record is rewritten.
    pub fn cell_kv_remove(&mut self, wx: i32, wy: i32, wz: i32, key: &str) -> bool {
        if !self.cell_kv_writable(wx, wy, wz) {
            return false;
        }
        let Some((s, lx, ly, lz)) = self.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        let removed = s.cell_kv_remove(lx, ly, lz, key);
        if removed {
            s.modified = true;
            let pos = petramond_math::math::IVec3::new(wx, wy, wz);
            // Same block-delta skip as `cell_kv_set`: a covering block delta's
            // drain-time KV snapshot already reflects the removal.
            if self.replication.replication_capture
                && !self.replication.block_delta_log.contains_key(&pos)
            {
                self.replication
                    .cell_kv_delta_log
                    .insert((pos, key.to_string()), None);
            }
            if petramond_world::block::kv_key_affects_mesh(key) {
                self.queue_dirty_meshes_sampling_cell(wx, wy, wz);
            }
            self.remark_state_key_bakes(wx, wy, wz, key);
        }
        removed
    }
}
