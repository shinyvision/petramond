use crate::world::{World, WorldSide};

impl<S: WorldSide> World<S> {
    pub fn cell_kv_set(&mut self, wx: i32, wy: i32, wz: i32, key: String, value: Vec<u8>) -> bool {
        if !self.data.cell_kv_writable(wx, wy, wz) {
            return false;
        }
        let pos = petramond_math::math::IVec3::new(wx, wy, wz);
        let capture = self
            .side
            .server()
            .is_some_and(|s| s.replication.wants_cell_kv_delta(pos));
        let log_value = capture.then(|| value.clone());
        let Some((s, lx, ly, lz)) = self.data.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        s.cell_kv_set(lx, ly, lz, key.clone(), value);
        s.modified = true;
        if let (Some(v), Some(server)) = (log_value, self.side.server_mut()) {
            server
                .replication
                .cell_kv_delta_log
                .insert((pos, key.clone()), Some(v));
        }
        if petramond_world::block::kv_key_affects_mesh(&key) {
            self.queue_dirty_meshes_sampling_cell(wx, wy, wz);
        }
        self.data.remark_state_key_bakes(wx, wy, wz, &key);
        true
    }
    pub fn cell_kv_remove(&mut self, wx: i32, wy: i32, wz: i32, key: &str) -> bool {
        if !self.data.cell_kv_writable(wx, wy, wz) {
            return false;
        }
        let Some((s, lx, ly, lz)) = self.data.chunk_at_world_mut(wx, wy, wz) else {
            return false;
        };
        let removed = s.cell_kv_remove(lx, ly, lz, key);
        if removed {
            s.modified = true;
            let pos = petramond_math::math::IVec3::new(wx, wy, wz);
            if let Some(server) = self.side.server_mut() {
                if server.replication.wants_cell_kv_delta(pos) {
                    server
                        .replication
                        .cell_kv_delta_log
                        .insert((pos, key.to_string()), None);
                }
            }
            if petramond_world::block::kv_key_affects_mesh(key) {
                self.queue_dirty_meshes_sampling_cell(wx, wy, wz);
            }
            self.data.remark_state_key_bakes(wx, wy, wz, key);
        }
        removed
    }
}
