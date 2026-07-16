//! Server-side replication delta log: the per-tick coalesced block/water
//! change capture and the sparse per-cell wire state it ships.

use crate::block::Block;
use crate::chunk::section_idx;

use super::store::World;

impl World {
    /// Turn the server-side replication log on/off (the server flips it on per
    /// tick while clients are connected). Turning capture off drops anything
    /// already logged, mirroring [`set_stream_event_capture`].
    ///
    /// [`set_stream_event_capture`]: Self::set_stream_event_capture
    pub(crate) fn set_replication_capture(&mut self, on: bool) {
        if !on {
            self.block_delta_log.clear();
        }
        self.replication_capture = on;
    }

    /// Drain this tick's coalesced block/water deltas (latest state per cell),
    /// sorted by cell so the wire batch is deterministic. Each delta's
    /// per-cell STATE is re-read here, at the drain: several placement funnels
    /// write their state maps AFTER the block write that announced the change
    /// (chest/furnace/torch insert their facing after `set_block_world`), so
    /// only the drain sees the whole tick's final state for the cell.
    pub(crate) fn take_block_deltas(&mut self) -> Vec<crate::net::protocol::BlockDelta> {
        let mut out: Vec<_> = self.block_delta_log.drain().map(|(_, d)| d).collect();
        out.sort_unstable_by_key(|d| (d.pos.x, d.pos.y, d.pos.z));
        for d in &mut out {
            // A section evicted since the write keeps the recorded state; the
            // recipient unloads it anyway.
            if self.section_loaded_at(d.pos.x, d.pos.y, d.pos.z) {
                d.state = self.cell_state_at(d.pos.x, d.pos.y, d.pos.z);
            }
        }
        out
    }

    /// Snapshot one cell's CURRENT content as a wire delta — the same shape
    /// [`record_block_delta`](Self::record_block_delta) logs, but on demand:
    /// the per-recipient corrective sync a use click that disagreed with the
    /// client's replica ships. `None` when the section is not loaded.
    pub(crate) fn block_delta_at(
        &self,
        pos: crate::mathh::IVec3,
    ) -> Option<crate::net::protocol::BlockDelta> {
        if !self.section_loaded_at(pos.x, pos.y, pos.z) {
            return None;
        }
        let block_id = self.chunk_block(pos.x, pos.y, pos.z);
        let water =
            (block_id == Block::Water.id()).then(|| self.water_meta_world(pos.x, pos.y, pos.z));
        Some(crate::net::protocol::BlockDelta {
            pos,
            block_id,
            water,
            state: self.cell_state_at(pos.x, pos.y, pos.z),
        })
    }

    /// Log the CURRENT content of one just-changed cell (called from the
    /// block-change announce choke point, after the write landed). `block_id`
    /// is the raw session id; `water` carries the meta byte iff the cell holds
    /// water. Latest write per cell per tick wins by construction; the sparse
    /// per-cell state is re-read once more at the drain (`take_block_deltas`).
    pub(super) fn record_block_delta(&mut self, wx: i32, wy: i32, wz: i32) {
        let block_id = self.chunk_block(wx, wy, wz);
        let water = (block_id == Block::Water.id()).then(|| self.water_meta_world(wx, wy, wz));
        let pos = crate::mathh::IVec3::new(wx, wy, wz);
        let state = self.cell_state_at(wx, wy, wz);
        self.block_delta_log.insert(
            pos,
            crate::net::protocol::BlockDelta {
                pos,
                block_id,
                water,
                state,
            },
        );
    }

    /// The cell's sparse per-cell block state as its wire [`CellState`], using
    /// the save codec's per-entry encodings — the delta-sized twin of the maps
    /// `Section::to_payload` ships whole. A cell carries at most one of these
    /// (`clear_on_block_change` wipes them all on any block write); a model
    /// cell folds its placed facing in.
    ///
    /// [`CellState`]: crate::net::protocol::CellState
    pub(super) fn cell_state_at(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<crate::net::protocol::CellState> {
        use crate::net::protocol::CellState;
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let s = self.sections.get(&pos)?;
        let cell = section_idx(lx, ly, lz) as u16;
        // A model cell may carry offset, facing, or both (the BASE cell of an
        // oriented multi-block records only its facing — offset [0,0,0] is
        // implicit); either one makes it a ModelCell on the wire.
        let model_off = s.model_cells().get(&cell).copied();
        let model_facing = s.model_facings().get(&cell).copied();
        if model_off.is_some() || model_facing.is_some() {
            return Some(CellState::ModelCell {
                off: model_off.unwrap_or([0, 0, 0]),
                facing: model_facing.unwrap_or_default().to_u8(),
            });
        }
        if let Some(d) = s.doors().get(&cell) {
            return Some(CellState::Door(d.encode()));
        }
        if let Some(st) = s.stair_states().get(&cell) {
            return Some(CellState::Stair(st.encode()));
        }
        if let Some(sl) = s.slab_states().get(&cell) {
            return Some(CellState::Slab([
                sl.encode_meta(),
                sl.layers[0].0,
                sl.layers[1].0,
            ]));
        }
        if let Some(a) = s.log_axes().get(&cell) {
            return Some(CellState::LogAxis(a.to_u8()));
        }
        if let Some(t) = s.torches().get(&cell) {
            return Some(CellState::Torch(t.to_u8()));
        }
        if let Some(f) = s.entity_facings().get(&cell) {
            // A furnace folds its lit state into the facing byte's high bit:
            // the replica's mesher flips the front texture from it, and a
            // lit-state delta otherwise carries nothing but this entry.
            let lit = s.furnaces().get(&cell).is_some_and(|f| f.is_lit());
            return Some(CellState::Facing(f.to_u8() | if lit { 0x80 } else { 0 }));
        }
        None
    }
}
