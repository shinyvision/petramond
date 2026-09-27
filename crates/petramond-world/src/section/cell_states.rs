use std::collections::{BTreeMap, HashMap};

use crate::block::{CellCodec, CellView, ShapeState};
use crate::block_model::ModelCellState;
#[cfg(any(test, feature = "test-support"))]
use crate::block_state::StairHalf;
use crate::block_state::{EntityFront, LogAxis, SlabState, StairState};
use crate::door::DoorState;
use crate::facing::Facing;
use crate::torch::TorchPlacement;
use crate::trapdoor::TrapdoorState;

use super::{CellMap, Section};

impl Section {
    #[inline]
    pub fn cell_state(&self, x: usize, y: usize, z: usize) -> ShapeState {
        self.states.cell_state(x, y, z)
    }

    pub fn set_cell_state(&mut self, x: usize, y: usize, z: usize, state: ShapeState) {
        self.states.set_cell_state(x, y, z, state);
        self.modified = true;
    }

    #[inline]
    pub fn cell_states(&self) -> &CellMap<ShapeState> {
        self.states.cell_states()
    }

    #[inline]
    pub fn state_of<T: CellView>(&self, x: usize, y: usize, z: usize) -> T {
        if T::owns(self.block(x, y, z)) {
            T::from_cell(self.states.cell_state(x, y, z))
        } else {
            T::from_cell(ShapeState::NONE)
        }
    }

    pub fn set_state_of<T: CellCodec>(&mut self, x: usize, y: usize, z: usize, v: &T) {
        self.states.set_cell_state(x, y, z, v.to_cell());
        self.modified = true;
    }

    #[inline]
    pub fn model_offset(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.state_of::<ModelCellState>(x, y, z).offset
    }

    #[inline]
    pub fn set_model_offset(&mut self, x: usize, y: usize, z: usize, offset: [u8; 3]) {
        let mut st = self.state_of::<ModelCellState>(x, y, z);
        st.offset = offset;
        self.set_state_of(x, y, z, &st);
        self.dirty = true;
    }

    #[inline]
    pub fn model_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.state_of::<ModelCellState>(x, y, z).facing
    }

    #[inline]
    pub fn set_model_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        let mut st = self.state_of::<ModelCellState>(x, y, z);
        st.facing = facing;
        self.set_state_of(x, y, z, &st);
        self.dirty = true;
    }

    #[inline]
    pub fn door_state(&self, x: usize, y: usize, z: usize) -> Option<DoorState> {
        self.state_of::<Option<DoorState>>(x, y, z)
    }

    pub fn set_door_state(&mut self, x: usize, y: usize, z: usize, state: DoorState) {
        self.set_state_of(x, y, z, &state);
    }

    #[inline]
    pub fn trapdoor_state(&self, x: usize, y: usize, z: usize) -> Option<TrapdoorState> {
        self.state_of::<Option<TrapdoorState>>(x, y, z)
    }

    pub fn set_trapdoor_state(&mut self, x: usize, y: usize, z: usize, state: TrapdoorState) {
        self.set_state_of(x, y, z, &state);
    }

    #[inline]
    pub fn stair_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.stair_state(x, y, z).facing
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_stair_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.set_stair_state(x, y, z, StairState::new(facing, StairHalf::Bottom));
    }

    #[inline]
    pub fn stair_state(&self, x: usize, y: usize, z: usize) -> StairState {
        self.state_of(x, y, z)
    }

    pub fn set_stair_state(&mut self, x: usize, y: usize, z: usize, state: StairState) {
        self.set_state_of(x, y, z, &state);
    }

    #[inline]
    pub fn slab_state(&self, x: usize, y: usize, z: usize) -> SlabState {
        self.state_of(x, y, z)
    }

    pub fn set_slab_state(&mut self, x: usize, y: usize, z: usize, state: SlabState) {
        self.set_state_of(x, y, z, &state);
    }

    #[inline]
    pub fn log_axis(&self, x: usize, y: usize, z: usize) -> LogAxis {
        self.state_of(x, y, z)
    }

    pub fn set_log_axis(&mut self, x: usize, y: usize, z: usize, axis: LogAxis) {
        self.set_state_of(x, y, z, &axis);
    }

    #[inline]
    pub fn torch_placement(&self, x: usize, y: usize, z: usize) -> TorchPlacement {
        self.state_of(x, y, z)
    }

    pub fn insert_torch(&mut self, x: usize, y: usize, z: usize, placement: TorchPlacement) {
        self.set_state_of(x, y, z, &placement);
    }

    #[inline]
    pub fn entity_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.state_of::<EntityFront>(x, y, z).0
    }

    pub fn insert_entity_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.set_state_of(x, y, z, &EntityFront(facing));
    }

    #[inline]
    pub fn cell_kv_get(&self, x: usize, y: usize, z: usize, key: &str) -> Option<&[u8]> {
        self.states.cell_kv_get(x, y, z, key)
    }

    pub fn cell_kv_set(&mut self, x: usize, y: usize, z: usize, key: String, value: Vec<u8>) {
        self.states.cell_kv_set(x, y, z, key, value);
    }

    pub fn cell_kv_remove(&mut self, x: usize, y: usize, z: usize, key: &str) -> bool {
        self.states.cell_kv_remove(x, y, z, key)
    }

    pub fn cell_kv(&self) -> &CellMap<BTreeMap<String, Vec<u8>>> {
        self.states.cell_kv()
    }

    pub fn cell_tint_map(&self) -> HashMap<u16, Vec<(crate::block::CellPart, [f32; 3])>> {
        let mut out: HashMap<u16, Vec<(crate::block::CellPart, [f32; 3])>> = HashMap::new();
        for (&idx, map) in self.states.cell_kv() {
            for (key, v) in map {
                let (base, part) = crate::block::split_part_kv_key(key);
                if base != crate::block::TINT_KV_KEY {
                    continue;
                }
                let Ok([r, g, b]) = <[u8; 3]>::try_from(v.as_slice()) else {
                    continue;
                };
                out.entry(idx)
                    .or_default()
                    .push((part, [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]));
            }
        }
        for parts in out.values_mut() {
            parts.sort_unstable_by_key(|&(part, _)| part);
        }
        out
    }

    pub fn cell_parts_map(&self) -> HashMap<u16, u32> {
        let mut out = HashMap::new();
        for (&idx, map) in self.states.cell_kv() {
            if let Some(v) = map.get(crate::block_model::PARTS_KV_KEY) {
                if let Ok(bytes) = <[u8; 4]>::try_from(v.as_slice()) {
                    out.insert(idx, u32::from_le_bytes(bytes));
                }
            }
        }
        out
    }

    pub fn cell_kv_take(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Option<BTreeMap<String, Vec<u8>>> {
        self.states.cell_kv_take(x, y, z)
    }

    pub fn cell_kv_restore(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        map: BTreeMap<String, Vec<u8>>,
    ) {
        self.states.cell_kv_restore(x, y, z, map);
    }
}
