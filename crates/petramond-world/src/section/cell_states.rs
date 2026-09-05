//! Typed fronts over the section's UNIFIED per-cell state store.
//!
//! Storage is one opaque map (`cell index -> ShapeState`); the two GENERIC
//! accessors below are the only codec-aware machinery — each typed one-liner
//! just names the view/codec type, whose byte vocabulary lives beside the
//! type in its owner's module (`CellView`/`CellCodec`). Every read gates on
//! `T::owns`, so a foreign cell's bytes decode as the type's defaults, never
//! garbage; presence semantics (a door's stored all-zero pose, a vertical
//! log's elided entry) live in each codec.

use std::collections::{BTreeMap, HashMap};

use crate::block::{CellCodec, CellView, ShapeState};
use crate::block_model::ModelCellState;
#[cfg(any(test, feature = "test-support"))]
use crate::block_state::StairHalf;
use crate::block_state::{EntityFront, LogAxis, SlabState, StairState};
use crate::door::DoorState;
use crate::facing::Facing;
use crate::torch::TorchPlacement;

use super::Section;

impl Section {
    /// The cell's raw opaque state — the seam/store read; typed consumers use
    /// the gated generic accessors below.
    #[inline]
    pub fn cell_state(&self, x: usize, y: usize, z: usize) -> ShapeState {
        self.states.cell_state(x, y, z)
    }

    /// Store a cell's raw opaque state (empty clears). The replication-apply
    /// write: a delta ships the state bytes verbatim and lands them here.
    pub fn set_cell_state(&mut self, x: usize, y: usize, z: usize, state: ShapeState) {
        self.states.set_cell_state(x, y, z, state);
        self.modified = true;
    }

    /// The whole unified state map (save codec, wire payload, light snapshot,
    /// mesh-pad capture, per-kind render collectors).
    #[inline]
    pub fn cell_states(&self) -> &HashMap<u16, ShapeState> {
        self.states.cell_states()
    }

    /// The cell's state decoded through `T`'s view, gated on `T::owns` — a
    /// foreign cell answers `T`'s default semantics.
    #[inline]
    pub fn state_of<T: CellView>(&self, x: usize, y: usize, z: usize) -> T {
        if T::owns(self.block(x, y, z)) {
            T::from_cell(self.states.cell_state(x, y, z))
        } else {
            T::from_cell(ShapeState::NONE)
        }
    }

    /// Store `v` through its codec (a default the codec elides clears the
    /// entry).
    pub fn set_state_of<T: CellCodec>(&mut self, x: usize, y: usize, z: usize, v: &T) {
        self.states.set_cell_state(x, y, z, v.to_cell());
        self.modified = true;
    }

    // --- Typed one-liner fronts (each is just a view/codec name) -----------

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
    pub fn stair_facing(&self, x: usize, y: usize, z: usize) -> Facing {
        self.stair_state(x, y, z).facing
    }

    /// Test convenience only: places a BOTTOM-half stair of `facing`. Lossy by
    /// construction (it cannot express a top half), which is why production
    /// writes `set_stair_state` with a full [`StairState`].
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

    /// The section's `petramond:tint` presentation entries as cell-index →
    /// the cell's per-[`CellPart`](crate::block::CellPart) multiply colors.
    /// Sparse (empty for almost every section) — collected once per mesh build
    /// so the per-face lookup is a probe only on sections that actually carry
    /// tints. Only well-formed 3-byte values count.
    ///
    /// A cell's list is sorted by part and almost always length 1 (part 0, the
    /// bare key), so the per-box part lookup is a scan over a handful of
    /// entries rather than a second hash.
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

    /// The section's `petramond:parts` entries as cell-index → the model
    /// block's per-instance visible-part mask. Sparse like the tint map, and
    /// collected once per mesh build for the same reason. A malformed value
    /// (not 4 bytes) is ignored, which shows as the base row rather than as a
    /// crash.
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

    /// Detach one cell's whole mod-KV map — the state-preserving half of a
    /// model-block swap (see `World::swap_block`).
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
}
