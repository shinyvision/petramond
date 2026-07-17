use std::collections::{BTreeMap, HashMap};

use crate::block_state::{LogAxis, SlabState, StairHalf, StairState};
use crate::door::DoorState;
use crate::facing::Facing;
use crate::torch::TorchPlacement;

use super::Section;

impl Section {
    #[inline]
    pub fn set_model_offset(&mut self, x: usize, y: usize, z: usize, offset: [u8; 3]) {
        self.states.set_model_offset(x, y, z, offset);
        self.dirty = true;
    }

    #[inline]
    pub fn model_offset(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.states.model_offset(x, y, z)
    }

    #[inline]
    pub fn set_model_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.states.set_model_facing(x, y, z, facing);
        self.dirty = true;
    }

    #[inline]
    pub fn model_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.states.model_facing(x, y, z)
    }

    #[inline]
    pub fn model_cells(&self) -> &HashMap<u16, [u8; 3]> {
        self.states.model_cells()
    }

    #[inline]
    pub fn model_facings(&self) -> &HashMap<u16, Facing> {
        self.states.model_facings()
    }

    #[inline]
    pub fn sapling_stage(&self, x: usize, y: usize, z: usize) -> u8 {
        self.states.sapling_stage(x, y, z)
    }

    pub fn set_sapling_stage(&mut self, x: usize, y: usize, z: usize, stage: u8) {
        self.states.set_sapling_stage(x, y, z, stage);
        self.modified = true;
    }

    #[inline]
    pub fn sapling_stages(&self) -> &HashMap<u16, u8> {
        self.states.sapling_stages()
    }

    #[inline]
    pub fn door_state(&self, x: usize, y: usize, z: usize) -> Option<DoorState> {
        self.states.door_state(x, y, z)
    }

    pub fn set_door_state(&mut self, x: usize, y: usize, z: usize, state: DoorState) {
        self.states.set_door_state(x, y, z, state);
        self.modified = true;
    }

    #[inline]
    pub fn doors(&self) -> &HashMap<u16, DoorState> {
        self.states.doors()
    }

    #[inline]
    pub fn stair_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.stair_state(x, y, z).facing
    }

    pub fn set_stair_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.set_stair_state(x, y, z, StairState::new(facing, StairHalf::Bottom));
    }

    #[inline]
    pub fn stair_state(&self, x: usize, y: usize, z: usize) -> StairState {
        self.states.stair_state(x, y, z)
    }

    pub fn set_stair_state(&mut self, x: usize, y: usize, z: usize, state: StairState) {
        self.states.set_stair_state(x, y, z, state);
        self.modified = true;
    }

    #[inline]
    pub fn log_axis(&self, x: usize, y: usize, z: usize) -> LogAxis {
        self.states.log_axis(x, y, z)
    }

    pub fn set_log_axis(&mut self, x: usize, y: usize, z: usize, axis: LogAxis) {
        self.states.set_log_axis(x, y, z, axis);
        self.modified = true;
    }

    #[inline]
    pub fn log_axes(&self) -> &HashMap<u16, LogAxis> {
        self.states.log_axes()
    }

    #[inline]
    /// A cell's mod KV entry, or `None` when the cell (or key) has none.
    pub fn cell_kv_get(&self, x: usize, y: usize, z: usize, key: &str) -> Option<&[u8]> {
        self.states.cell_kv_get(x, y, z, key)
    }

    /// Store a cell mod KV entry. Does NOT set `modified` — the world-level
    /// wrapper owns that (mirroring the block-entity insert pattern).
    pub fn cell_kv_set(&mut self, x: usize, y: usize, z: usize, key: String, value: Vec<u8>) {
        self.states.cell_kv_set(x, y, z, key, value);
    }

    /// Remove a cell mod KV entry; returns whether it was present. An inner
    /// map emptied by the removal is dropped whole, so the save codec's
    /// has-cell-kv flag clears once the last entry goes (the stale-record
    /// guard pattern).
    pub fn cell_kv_remove(&mut self, x: usize, y: usize, z: usize, key: &str) -> bool {
        self.states.cell_kv_remove(x, y, z, key)
    }

    /// The whole per-cell mod KV map, for the save codec.
    pub fn cell_kv(&self) -> &HashMap<u16, BTreeMap<String, Vec<u8>>> {
        self.states.cell_kv()
    }

    /// Detach one cell's whole mod-KV map — the state-preserving half of a
    /// model-block swap (see `World::swap_model_block`).
    pub fn cell_kv_take(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Option<BTreeMap<String, Vec<u8>>> {
        self.states.cell_kv_take(x, y, z)
    }

    /// Re-attach a map detached by [`cell_kv_take`](Self::cell_kv_take).
    pub fn cell_kv_restore(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        map: BTreeMap<String, Vec<u8>>,
    ) {
        self.states.cell_kv_restore(x, y, z, map);
    }

    pub fn stair_states(&self) -> &HashMap<u16, StairState> {
        self.states.stair_states()
    }

    #[inline]
    pub fn slab_state(&self, x: usize, y: usize, z: usize) -> SlabState {
        self.states.slab_state(x, y, z)
    }

    pub fn set_slab_state(&mut self, x: usize, y: usize, z: usize, state: SlabState) {
        self.states.set_slab_state(x, y, z, state);
        self.modified = true;
    }

    pub fn slab_states(&self) -> &HashMap<u16, SlabState> {
        self.states.slab_states()
    }

    #[inline]
    pub fn torch_placement(&self, x: usize, y: usize, z: usize) -> TorchPlacement {
        self.states.torch_placement(x, y, z)
    }

    pub fn insert_torch(&mut self, x: usize, y: usize, z: usize, placement: TorchPlacement) {
        self.states.insert_torch(x, y, z, placement);
        self.modified = true;
    }

    pub fn take_torch(&mut self, x: usize, y: usize, z: usize) {
        if self.states.take_torch(x, y, z) {
            self.modified = true;
        }
    }

    #[inline]
    pub fn torches(&self) -> &HashMap<u16, TorchPlacement> {
        self.states.torches()
    }
}
