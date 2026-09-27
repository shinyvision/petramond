use crate::world::WorldData;
use crate::world::{ServerWorld, World, WorldSide};
use petramond_world::block::Block;
use petramond_world::chunk::section_idx;

impl ServerWorld {
    pub fn set_replication_capture(&mut self, on: bool) {
        if !on {
            self.side.replication.block_delta_log.clear();
            self.side.replication.cell_kv_delta_log.clear();
        }
        self.side.replication.replication_capture = on;
    }

    pub fn take_cell_kv_deltas(&mut self) -> Vec<crate::world::replication::CellKvDelta> {
        let mut out: Vec<_> = self
            .side
            .replication
            .cell_kv_delta_log
            .drain()
            .map(|((pos, key), value)| crate::world::replication::CellKvDelta { pos, key, value })
            .collect();
        out.sort_unstable_by(|a, b| {
            (a.pos.x, a.pos.y, a.pos.z, &a.key).cmp(&(b.pos.x, b.pos.y, b.pos.z, &b.key))
        });
        out
    }

    pub fn take_block_deltas(&mut self) -> Vec<crate::world::replication::BlockDelta> {
        let mut out: Vec<_> = self
            .side
            .replication
            .block_delta_log
            .drain()
            .map(|(_, d)| d)
            .collect();
        out.sort_unstable_by_key(|d| (d.pos.x, d.pos.y, d.pos.z));
        for d in &mut out {
            if self.data.section_loaded_at(d.pos.x, d.pos.y, d.pos.z) {
                d.state = self.cell_state_at(d.pos.x, d.pos.y, d.pos.z);
                d.cell_kv = self.cell_kv_map_at(d.pos.x, d.pos.y, d.pos.z);
            }
        }
        out
    }
}

impl<S: WorldSide> World<S> {
    fn cell_kv_map_at(&self, wx: i32, wy: i32, wz: i32) -> Vec<(String, Vec<u8>)> {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(wx, wy, wz) else {
            return Vec::new();
        };
        let Some(s) = self.data.sections.get(&pos) else {
            return Vec::new();
        };
        let cell = section_idx(lx, ly, lz) as u16;
        s.cell_kv()
            .get(&cell)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn block_delta_at(
        &self,
        pos: petramond_math::math::IVec3,
    ) -> Option<crate::world::replication::BlockDelta> {
        if !self.data.section_loaded_at(pos.x, pos.y, pos.z) {
            return None;
        }
        let block_id = self.data.chunk_block(pos.x, pos.y, pos.z);
        let fluid = Block::from_id(block_id)
            .is_fluid()
            .then(|| self.data.fluid_meta_world(pos.x, pos.y, pos.z));
        Some(crate::world::replication::BlockDelta {
            pos,
            block_id,
            fluid,
            state: self.cell_state_at(pos.x, pos.y, pos.z),
            cell_kv: self.cell_kv_map_at(pos.x, pos.y, pos.z),
        })
    }

    pub(super) fn record_block_delta(&mut self, wx: i32, wy: i32, wz: i32) {
        if !self
            .side
            .server()
            .is_some_and(|s| s.replication.replication_capture)
        {
            return;
        }
        let block_id = self.data.chunk_block(wx, wy, wz);
        let fluid = Block::from_id(block_id)
            .is_fluid()
            .then(|| self.data.fluid_meta_world(wx, wy, wz));
        let pos = petramond_math::math::IVec3::new(wx, wy, wz);
        let state = self.cell_state_at(wx, wy, wz);
        let Some(server) = self.side.server_mut() else {
            return;
        };
        server
            .replication
            .cell_kv_delta_log
            .retain(|(p, _), _| *p != pos);
        server.replication.block_delta_log.insert(
            pos,
            crate::world::replication::BlockDelta {
                pos,
                block_id,
                fluid,
                state,
                cell_kv: Vec::new(),
            },
        );
    }

    pub(super) fn cell_state_at(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<petramond_world::block::ShapeState> {
        let (pos, lx, ly, lz) = WorldData::split_world(wx, wy, wz)?;
        let s = self.data.sections.get(&pos)?;
        s.cell_states()
            .get(&(section_idx(lx, ly, lz) as u16))
            .copied()
    }
}
