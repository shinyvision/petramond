//! Centralized per-cell block state owned by a [`Section`](crate::section::Section).
//!
//! The block id buffer remains dense and minimal (`u8` per cell). Runtime state that
//! changes how a placed block behaves or renders lives here instead of in scattered
//! section fields. Water keeps a dense optional buffer because it can fill whole
//! sections; rarer block states stay sparse and keyed by `section_idx` (`u16`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::block::Block;
use crate::chunk::SECTION_VOLUME;
use crate::door::DoorState;
use crate::furnace::Facing;
use crate::torch::TorchPlacement;

#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum StairHalf {
    /// Right-side-up stair: full lower slab plus upper back half.
    #[default]
    Bottom = 0,
    /// Upside-down stair: full upper slab plus lower back half.
    Top = 1,
}

impl StairHalf {
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        if v & 0b1 != 0 {
            Self::Top
        } else {
            Self::Bottom
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StairState {
    /// The low/open horizontal side of the stair.
    pub facing: Facing,
    pub half: StairHalf,
}

impl StairState {
    #[inline]
    pub fn new(facing: Facing, half: StairHalf) -> Self {
        Self { facing, half }
    }

    #[inline]
    pub fn encode(self) -> u8 {
        self.facing.to_u8() | ((self.half as u8) << 2)
    }

    #[inline]
    pub fn decode(v: u8) -> Self {
        Self {
            facing: Facing::from_u8(v & 0b11),
            half: StairHalf::from_u8((v >> 2) & 0b1),
        }
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SlabSplit {
    X = 0,
    #[default]
    Y = 1,
    Z = 2,
}

impl SlabSplit {
    #[inline]
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::X,
            2 => Self::Z,
            _ => Self::Y,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlabState {
    pub split: SlabSplit,
    /// Slot 0 is the negative/lower half of the split axis, slot 1 the
    /// positive/upper half. `Air` means that slot is empty.
    pub layers: [Block; 2],
}

impl Default for SlabState {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl SlabState {
    pub const EMPTY: Self = Self {
        split: SlabSplit::Y,
        layers: [Block::Air, Block::Air],
    };

    #[inline]
    pub fn single(split: SlabSplit, slot: usize, block: Block) -> Self {
        let mut layers = [Block::Air, Block::Air];
        layers[slot.min(1)] = block;
        Self { split, layers }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.layers[0] == Block::Air && self.layers[1] == Block::Air
    }

    #[inline]
    pub fn is_full(self) -> bool {
        self.layers[0] != Block::Air && self.layers[1] != Block::Air
    }

    #[inline]
    pub fn mask(self) -> u8 {
        (u8::from(self.layers[0] != Block::Air)) | (u8::from(self.layers[1] != Block::Air) << 1)
    }

    #[inline]
    pub fn block_in_slot(self, slot: usize) -> Option<Block> {
        let block = self.layers[slot.min(1)];
        (block != Block::Air).then_some(block)
    }

    #[inline]
    pub fn with_slot(mut self, slot: usize, block: Block) -> Option<Self> {
        let slot = slot.min(1);
        if self.layers[slot] != Block::Air {
            return None;
        }
        self.layers[slot] = block;
        Some(self)
    }

    #[inline]
    pub fn encode_meta(self) -> u8 {
        self.split.to_u8() | (self.mask() << 2)
    }

    #[inline]
    pub fn decode(meta: u8, a: Block, b: Block) -> Self {
        let split = SlabSplit::from_u8(meta & 0b11);
        let mask = (meta >> 2) & 0b11;
        Self {
            split,
            layers: [
                if mask & 0b01 != 0 { a } else { Block::Air },
                if mask & 0b10 != 0 { b } else { Block::Air },
            ],
        }
    }
}

#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LogAxis {
    X = 0,
    #[default]
    Y = 1,
    Z = 2,
}

impl LogAxis {
    #[inline]
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::X,
            2 => Self::Z,
            _ => Self::Y,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum HeldBlockState {
    #[default]
    None,
    Stair(StairState),
    Slab(SlabState),
    Log(LogAxis),
}

#[derive(Clone, Default)]
pub(crate) struct BlockStates {
    water: Option<Arc<[u8]>>,
    torches: HashMap<u16, TorchPlacement>,
    model_cells: HashMap<u16, [u8; 3]>,
    model_facings: HashMap<u16, Facing>,
    sapling_stages: HashMap<u16, u8>,
    doors: HashMap<u16, DoorState>,
    stairs: HashMap<u16, StairState>,
    slabs: HashMap<u16, SlabState>,
    log_axes: HashMap<u16, LogAxis>,
    cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
}

impl BlockStates {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_saved(
        water: Option<Box<[u8]>>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        sapling_stages: HashMap<u16, u8>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
    ) -> Self {
        Self {
            water: water.map(Arc::from),
            torches,
            model_cells,
            model_facings,
            sapling_stages,
            doors,
            stairs,
            slabs,
            log_axes,
            cell_kv,
        }
    }

    #[inline]
    pub(crate) fn water_arc(&self) -> Option<Arc<[u8]>> {
        self.water.clone()
    }

    #[inline]
    pub(crate) fn water_slice(&self) -> Option<&[u8]> {
        self.water.as_deref()
    }

    #[inline]
    pub(crate) fn water_meta(&self, idx: usize) -> u8 {
        match &self.water {
            Some(w) => w[idx],
            None => 0,
        }
    }

    #[inline]
    pub(crate) fn clear_water_meta(&mut self, idx: usize) {
        if let Some(w) = self.water.as_mut() {
            Arc::make_mut(w)[idx] = 0;
        }
    }

    pub(crate) fn store_water_meta(&mut self, idx: usize, meta: u8) {
        if meta == 0 {
            self.clear_water_meta(idx);
            return;
        }
        let w = self
            .water
            .get_or_insert_with(|| vec![0u8; SECTION_VOLUME].into());
        Arc::make_mut(w)[idx] = meta;
    }

    #[inline]
    pub(crate) fn clear_on_block_change(&mut self, idx: usize) {
        self.clear_water_meta(idx);
        self.clear_model_cell(idx);
        self.clear_sapling_stage(idx);
        self.clear_door(idx);
        self.clear_stair(idx);
        self.clear_slab(idx);
        self.clear_log_axis(idx);
        self.clear_torch(idx);
        // Mod cell KV is per-BLOCK state like everything above: a broken
        // machine's burn state must die with the block — air holds no data.
        // (A same-footprint model swap that must KEEP the state carries it
        // across explicitly — see `World::swap_model_block`. A disabled mod's
        // KV is untouched by this: its sections load their KV wholesale, not
        // through per-cell block writes.)
        self.cell_kv.remove(&(idx as u16));
    }

    #[inline]
    fn key(x: usize, y: usize, z: usize) -> u16 {
        crate::chunk::section_idx(x, y, z) as u16
    }

    #[inline]
    fn clear_model_cell(&mut self, idx: usize) {
        let key = idx as u16;
        self.model_cells.remove(&key);
        self.model_facings.remove(&key);
    }

    #[inline]
    pub(crate) fn set_model_offset(&mut self, x: usize, y: usize, z: usize, offset: [u8; 3]) {
        self.model_cells.insert(Self::key(x, y, z), offset);
    }

    #[inline]
    pub(crate) fn model_offset(&self, x: usize, y: usize, z: usize) -> [u8; 3] {
        self.model_cells
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or([0, 0, 0])
    }

    #[inline]
    pub(crate) fn set_model_facing(&mut self, x: usize, y: usize, z: usize, facing: Facing) {
        self.model_facings.insert(Self::key(x, y, z), facing);
    }

    #[inline]
    pub(crate) fn model_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.model_facings
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or(crate::block_model::DEFAULT_MODEL_FACING)
    }

    #[inline]
    pub(crate) fn model_cells(&self) -> &HashMap<u16, [u8; 3]> {
        &self.model_cells
    }

    #[inline]
    pub(crate) fn model_facings(&self) -> &HashMap<u16, Facing> {
        &self.model_facings
    }

    #[inline]
    pub(crate) fn sapling_stage(&self, x: usize, y: usize, z: usize) -> u8 {
        self.sapling_stages
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or(0)
    }

    pub(crate) fn set_sapling_stage(&mut self, x: usize, y: usize, z: usize, stage: u8) {
        let key = Self::key(x, y, z);
        if stage == 0 {
            self.sapling_stages.remove(&key);
        } else {
            self.sapling_stages.insert(key, stage);
        }
    }

    #[inline]
    fn clear_sapling_stage(&mut self, idx: usize) {
        self.sapling_stages.remove(&(idx as u16));
    }

    #[inline]
    pub(crate) fn sapling_stages(&self) -> &HashMap<u16, u8> {
        &self.sapling_stages
    }

    #[inline]
    pub(crate) fn door_state(&self, x: usize, y: usize, z: usize) -> Option<DoorState> {
        self.doors.get(&Self::key(x, y, z)).copied()
    }

    #[inline]
    pub(crate) fn set_door_state(&mut self, x: usize, y: usize, z: usize, state: DoorState) {
        self.doors.insert(Self::key(x, y, z), state);
    }

    #[inline]
    fn clear_door(&mut self, idx: usize) {
        self.doors.remove(&(idx as u16));
    }

    #[inline]
    pub(crate) fn doors(&self) -> &HashMap<u16, DoorState> {
        &self.doors
    }

    #[inline]
    pub(crate) fn stair_state(&self, x: usize, y: usize, z: usize) -> StairState {
        self.stairs
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn set_stair_state(&mut self, x: usize, y: usize, z: usize, state: StairState) {
        self.stairs.insert(Self::key(x, y, z), state);
    }

    #[inline]
    fn clear_stair(&mut self, idx: usize) {
        self.stairs.remove(&(idx as u16));
    }

    #[inline]
    pub(crate) fn stair_states(&self) -> &HashMap<u16, StairState> {
        &self.stairs
    }

    #[inline]
    pub(crate) fn slab_state(&self, x: usize, y: usize, z: usize) -> SlabState {
        self.slabs
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn set_slab_state(&mut self, x: usize, y: usize, z: usize, state: SlabState) {
        let key = Self::key(x, y, z);
        if state.is_empty() {
            self.slabs.remove(&key);
        } else {
            self.slabs.insert(key, state);
        }
    }

    #[inline]
    fn clear_slab(&mut self, idx: usize) {
        self.slabs.remove(&(idx as u16));
    }

    #[inline]
    pub(crate) fn slab_states(&self) -> &HashMap<u16, SlabState> {
        &self.slabs
    }

    #[inline]
    pub(crate) fn log_axis(&self, x: usize, y: usize, z: usize) -> LogAxis {
        self.log_axes
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn set_log_axis(&mut self, x: usize, y: usize, z: usize, axis: LogAxis) {
        let key = Self::key(x, y, z);
        if axis == LogAxis::Y {
            self.log_axes.remove(&key);
        } else {
            self.log_axes.insert(key, axis);
        }
    }

    #[inline]
    fn clear_log_axis(&mut self, idx: usize) {
        self.log_axes.remove(&(idx as u16));
    }

    #[inline]
    pub(crate) fn log_axes(&self) -> &HashMap<u16, LogAxis> {
        &self.log_axes
    }

    #[inline]
    pub(crate) fn torch_placement(&self, x: usize, y: usize, z: usize) -> TorchPlacement {
        self.torches
            .get(&Self::key(x, y, z))
            .copied()
            .unwrap_or_default()
    }

    #[inline]
    pub(crate) fn insert_torch(&mut self, x: usize, y: usize, z: usize, placement: TorchPlacement) {
        self.torches.insert(Self::key(x, y, z), placement);
    }

    #[inline]
    pub(crate) fn take_torch(&mut self, x: usize, y: usize, z: usize) -> bool {
        self.torches.remove(&Self::key(x, y, z)).is_some()
    }

    #[inline]
    fn clear_torch(&mut self, idx: usize) {
        self.torches.remove(&(idx as u16));
    }

    #[inline]
    pub(crate) fn torches(&self) -> &HashMap<u16, TorchPlacement> {
        &self.torches
    }

    pub(crate) fn cell_kv_get(&self, x: usize, y: usize, z: usize, key: &str) -> Option<&[u8]> {
        self.cell_kv
            .get(&Self::key(x, y, z))?
            .get(key)
            .map(Vec::as_slice)
    }

    pub(crate) fn cell_kv_set(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        key: String,
        value: Vec<u8>,
    ) {
        self.cell_kv
            .entry(Self::key(x, y, z))
            .or_default()
            .insert(key, value);
    }

    pub(crate) fn cell_kv_remove(&mut self, x: usize, y: usize, z: usize, key: &str) -> bool {
        let idx = Self::key(x, y, z);
        let Some(map) = self.cell_kv.get_mut(&idx) else {
            return false;
        };
        let removed = map.remove(key).is_some();
        if map.is_empty() {
            self.cell_kv.remove(&idx);
        }
        removed
    }

    #[inline]
    pub(crate) fn cell_kv(&self) -> &HashMap<u16, BTreeMap<String, Vec<u8>>> {
        &self.cell_kv
    }

    /// Detach one cell's whole mod-KV map, for a state-PRESERVING block swap
    /// (`set_block` clears cell KV like every other per-cell state, so a swap
    /// that must keep it takes it out first and restores it after).
    pub(crate) fn cell_kv_take(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
    ) -> Option<BTreeMap<String, Vec<u8>>> {
        self.cell_kv.remove(&Self::key(x, y, z))
    }

    /// Re-attach a map detached by [`cell_kv_take`](Self::cell_kv_take).
    pub(crate) fn cell_kv_restore(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        map: BTreeMap<String, Vec<u8>>,
    ) {
        if !map.is_empty() {
            self.cell_kv.insert(Self::key(x, y, z), map);
        }
    }
}
