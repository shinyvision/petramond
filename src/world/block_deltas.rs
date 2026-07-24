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
            self.cell_kv_delta_log.clear();
        }
        self.replication_capture = on;
    }

    /// Drain this tick's coalesced per-cell mod KV changes (latest value per
    /// `(pos, key)`), sorted so the wire batch is deterministic. The
    /// recipient applies them AFTER the same batch's block deltas — a block
    /// write wipes the cell's KV on both sides, so a same-tick
    /// write-block-then-KV sequence survives in order.
    pub(crate) fn take_cell_kv_deltas(&mut self) -> Vec<crate::net::protocol::CellKvDelta> {
        let mut out: Vec<_> = self
            .cell_kv_delta_log
            .drain()
            .map(|((pos, key), value)| crate::net::protocol::CellKvDelta { pos, key, value })
            .collect();
        out.sort_unstable_by(|a, b| {
            (a.pos.x, a.pos.y, a.pos.z, &a.key).cmp(&(b.pos.x, b.pos.y, b.pos.z, &b.key))
        });
        out
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
                d.cell_kv = self.cell_kv_map_at(d.pos.x, d.pos.y, d.pos.z);
            }
        }
        out
    }

    /// Snapshot a cell's whole mod KV map for the wire (sorted — BTreeMap
    /// iteration), empty for the common no-KV cell. Every delta carries it:
    /// the replica's apply wipes the cell's KV like a server-side write, so a
    /// delta without it would erase KV the server still holds (the corrective
    /// delta is a snapshot of an UNCHANGED cell — the gray-dye bug).
    fn cell_kv_map_at(&self, wx: i32, wy: i32, wz: i32) -> Vec<(String, Vec<u8>)> {
        let Some((pos, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return Vec::new();
        };
        let Some(s) = self.sections.get(&pos) else {
            return Vec::new();
        };
        let cell = section_idx(lx, ly, lz) as u16;
        s.cell_kv()
            .get(&cell)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
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
            cell_kv: self.cell_kv_map_at(pos.x, pos.y, pos.z),
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
        // KV deltas already logged for this cell are STALE: the block write
        // wiped the cell's KV, so replaying them after this delta would
        // resurrect keys the server no longer holds (the replica applies KV
        // deltas AFTER block deltas). Anything the cell still holds — or is
        // written later this tick — ships in this delta's drain-time KV
        // snapshot instead (`cell_kv_set` skips the log while this delta is
        // pending).
        self.cell_kv_delta_log.retain(|(p, _), _| *p != pos);
        self.block_delta_log.insert(
            pos,
            crate::net::protocol::BlockDelta {
                pos,
                block_id,
                water,
                state,
                // Re-read at the drain, like `state` (writes after the
                // announce — the fill's KV — must reach the same delta).
                cell_kv: Vec::new(),
            },
        );
    }

    /// The cell's opaque per-cell block state for the wire — the delta-sized
    /// twin of the unified list `Section::to_payload` ships whole. Verbatim
    /// store bytes; `None` when the cell carries no state.
    pub(super) fn cell_state_at(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> Option<crate::block::ShapeState> {
        let (pos, lx, ly, lz) = Self::split_world(wx, wy, wz)?;
        let s = self.sections.get(&pos)?;
        s.cell_states()
            .get(&(section_idx(lx, ly, lz) as u16))
            .copied()
    }
}
