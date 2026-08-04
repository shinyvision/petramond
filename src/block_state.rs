//! Centralized per-cell block state owned by a [`Section`](crate::section::Section).
//!
//! The block id buffer remains dense and minimal (`u8` per cell). Runtime state that
//! changes how a placed block behaves or renders lives here instead of in scattered
//! section fields. Water keeps a dense optional buffer because it can fill whole
//! sections; rarer block states stay sparse and keyed by `section_idx` (`u16`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::block::{Block, ShapeState};
use crate::chunk::SECTION_VOLUME;
use crate::facing::Facing;
use crate::wire_enum::wire_enum;

wire_enum! {
    pub enum StairHalf: u8 {
        /// Right-side-up stair: full lower slab plus upper back half.
        Bottom = 0,
        /// Upside-down stair: full upper slab plus lower back half.
        Top = 1,
    }
    default Bottom
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
        self.facing.to_u8() | (self.half.to_u8() << 2)
    }

    #[inline]
    pub fn decode(v: u8) -> Self {
        Self {
            facing: Facing::from_u8(v & 0b11),
            half: StairHalf::from_u8((v >> 2) & 0b1),
        }
    }
}

wire_enum! {
    pub enum SlabSplit: u8 {
        X = 0,
        Y = 1,
        Z = 2,
    }
    default Y
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

impl crate::block::CellView for StairState {
    fn owns(block: Block) -> bool {
        crate::stair::is_stair(block)
    }
    fn from_cell(s: ShapeState) -> Self {
        Self::decode(s.byte(0))
    }
}
impl crate::block::CellCodec for StairState {
    /// The PLACED bits only (byte 0); the refine cascade appends the corner
    /// byte ([`crate::stair::StairShape`] is its read view).
    fn to_cell(&self) -> ShapeState {
        ShapeState::new(&[self.encode()])
    }
}

impl crate::block::CellView for SlabState {
    fn owns(block: Block) -> bool {
        crate::slab::is_slab(block)
    }
    /// RAW (un-normalized) — readers normalize with the cell's block.
    fn from_cell(s: ShapeState) -> Self {
        if s.is_empty() {
            return SlabState::EMPTY;
        }
        Self::decode(
            s.byte(0),
            Block::from_id(s.id_at(1)),
            Block::from_id(s.id_at(3)),
        )
    }
}
impl crate::block::CellCodec for SlabState {
    fn to_cell(&self) -> ShapeState {
        if self.is_empty() {
            // An empty stack clears its entry (the cell stops being a slab).
            return ShapeState::NONE;
        }
        // The two layer slots are BLOCK IDS — two bytes each, declared
        // through the id mask so the save palette / net transport rewrite them
        // generically.
        let [a_lo, a_hi] = ShapeState::id_bytes(self.layers[0].id());
        let [b_lo, b_hi] = ShapeState::id_bytes(self.layers[1].id());
        ShapeState::with_ids(&[self.encode_meta(), a_lo, a_hi, b_lo, b_hi], 0b0_1010)
    }
}

wire_enum! {
    pub enum LogAxis: u8 {
        X = 0,
        Y = 1,
        Z = 2,
    }
    default Y
}

impl crate::block::CellView for LogAxis {
    fn owns(block: Block) -> bool {
        block.is_log()
    }
    fn from_cell(s: ShapeState) -> Self {
        // The default vertical axis is NOT the zero byte (`X` is 0), so
        // absence is checked explicitly.
        if s.is_empty() {
            return LogAxis::default();
        }
        LogAxis::from_u8(s.byte(0))
    }
}
impl crate::block::CellCodec for LogAxis {
    fn to_cell(&self) -> ShapeState {
        if *self == LogAxis::Y {
            // Vertical stays stateless — worldgen forests never pay a record.
            return ShapeState::NONE;
        }
        ShapeState::new(&[self.to_u8()])
    }
}

/// A directional block-entity's front (chest/furnace) as cell state. Cell
/// state like every other orientation — it survives a block-row swap only
/// through `World::swap_block_skin`'s explicit carry.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EntityFront(pub Facing);

impl crate::block::CellView for EntityFront {
    fn owns(block: Block) -> bool {
        block.directional_view()
    }
    fn from_cell(s: ShapeState) -> Self {
        Self(Facing::from_u8(s.byte(0)))
    }
}
impl crate::block::CellCodec for EntityFront {
    fn to_cell(&self) -> ShapeState {
        ShapeState::new(&[self.0.to_u8()])
    }
}

// `pub`, not `pub(crate)`, to match the `pub` fields of `render::HeldItemView`
// / `HeldItemFrame` that carry it — `block_state` is a private module, so this
// is still crate-visible either way, and the mismatch was a live
// `private_interfaces` warning.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum HeldBlockState {
    #[default]
    None,
    Stair(StairState),
    Slab(SlabState),
    Log(LogAxis),
}

/// A lazily-shared empty map, so absent sparse state can hand out `&HashMap`
/// without allocating. Each expansion site owns one static empty map.
macro_rules! empty_map {
    ($V:ty) => {{
        static EMPTY: std::sync::LazyLock<HashMap<u16, $V>> =
            std::sync::LazyLock::new(HashMap::new);
        &*EMPTY
    }};
}
pub(crate) use empty_map;

/// The two sparse per-cell stores, boxed behind `Option` in [`BlockStates`]:
/// the common generated section carries neither, and inline map headers
/// dominated `size_of::<Section>()`.
#[derive(Clone, Default)]
struct SparseStates {
    /// THE unified per-cell block state: one opaque [`ShapeState`] per
    /// stateful cell (stair facing, slab layers, door pose, torch mount, log
    /// axis, model offset+facing, chest/furnace front — every former typed
    /// map). The bytes are meaningful only to the owning family/behavior's
    /// codec (`crate::block::encode_*` / `decode_*`); the store, the save
    /// record, and the replication delta never interpret them.
    cell_states: HashMap<u16, ShapeState>,
    cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
}

impl SparseStates {
    fn is_empty(&self) -> bool {
        self.cell_states.is_empty() && self.cell_kv.is_empty()
    }
}

#[derive(Clone, Default)]
pub(crate) struct BlockStates {
    water: Option<Arc<[u8]>>,
    /// Count of nonzero water-meta cells (water mid-flow). O(1) "anything
    /// flowing?" for the streamed-water kick; the buffer is dropped when the
    /// last cell settles, so `water` is `Some` iff this is nonzero.
    flowing_count: u16,
    /// Allocated on the first sparse-state insert; `None` for the common section.
    sparse: Option<Box<SparseStates>>,
}

impl BlockStates {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline]
    fn sparse_mut(&mut self) -> &mut SparseStates {
        self.sparse.get_or_insert_default()
    }

    pub(crate) fn from_shared(
        water: Option<Arc<[u8]>>,
        cell_states: HashMap<u16, ShapeState>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
    ) -> Self {
        let sparse = SparseStates {
            cell_states,
            cell_kv,
        };
        let flowing_count = water
            .as_deref()
            .map_or(0, |w| w.iter().filter(|&&m| m != 0).count() as u16);
        Self {
            water: water.filter(|_| flowing_count > 0),
            flowing_count,
            sparse: (!sparse.is_empty()).then(|| Box::new(sparse)),
        }
    }

    #[inline]
    pub(crate) fn water_arc(&self) -> Option<Arc<[u8]>> {
        self.water.clone()
    }

    /// `(water buffer ptr, water len, sparse heap bytes)` for the memory census.
    pub(crate) fn memory_parts(&self) -> (Option<usize>, usize, u64) {
        let sparse = self.sparse.as_ref().map_or(0, |s| {
            let states = (s.cell_states.len() * (2 + std::mem::size_of::<ShapeState>() + 1)) as u64;
            let kv: u64 = s
                .cell_kv
                .values()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.capacity() + v.capacity() + 48) as u64)
                        .sum::<u64>()
                })
                .sum();
            std::mem::size_of::<SparseStates>() as u64 + states * 8 / 7 + kv
        });
        (
            self.water.as_ref().map(|w| w.as_ptr() as usize),
            self.water.as_ref().map_or(0, |w| w.len()),
            sparse,
        )
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
        // Read before `make_mut`: clearing an already-settled cell (the common
        // block edit) must not clone a buffer a mesh job still shares.
        let Some(w) = self.water.as_mut() else { return };
        if w[idx] == 0 {
            return;
        }
        Arc::make_mut(w)[idx] = 0;
        self.flowing_count -= 1;
        if self.flowing_count == 0 {
            self.water = None;
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
        let cell = &mut Arc::make_mut(w)[idx];
        if *cell == 0 {
            self.flowing_count += 1;
        }
        *cell = meta;
    }

    /// Whether any cell holds nonzero water-flow meta (water mid-flow).
    #[inline]
    pub(crate) fn has_flowing(&self) -> bool {
        self.flowing_count > 0
    }

    #[inline]
    pub(crate) fn clear_on_block_change(&mut self, idx: usize) {
        self.clear_water_meta(idx);
        let Some(s) = self.sparse.as_deref_mut() else {
            return;
        };
        let key = idx as u16;
        s.cell_states.remove(&key);
        // Mod cell KV is per-BLOCK state like the cell state above: a broken
        // machine's burn state must die with the block — air holds no data.
        // (A block-row swap that must KEEP its per-cell state carries it
        // across explicitly — see `World::swap_block_skin` /
        // `World::swap_model_block`. A disabled mod's KV is untouched by
        // this: its sections load their KV wholesale, not through per-cell
        // block writes.)
        s.cell_kv.remove(&key);
    }

    #[inline]
    fn key(x: usize, y: usize, z: usize) -> u16 {
        crate::chunk::section_idx(x, y, z) as u16
    }

    /// The cell's opaque per-cell block state ([`ShapeState::NONE`] when it
    /// carries none). The store never interprets the bytes.
    #[inline]
    pub(crate) fn cell_state(&self, x: usize, y: usize, z: usize) -> ShapeState {
        match &self.sparse {
            Some(s) => s
                .cell_states
                .get(&Self::key(x, y, z))
                .copied()
                .unwrap_or(ShapeState::NONE),
            None => ShapeState::NONE,
        }
    }

    /// Store a cell's opaque state; an EMPTY state removes the entry. NOTE:
    /// presence is meaningful (a door's all-zero pose byte is a valid stored
    /// state) — only a zero-LENGTH state clears.
    #[inline]
    pub(crate) fn set_cell_state(&mut self, x: usize, y: usize, z: usize, state: ShapeState) {
        let key = Self::key(x, y, z);
        if state.is_empty() {
            if let Some(s) = self.sparse.as_deref_mut() {
                s.cell_states.remove(&key);
            }
        } else {
            self.sparse_mut().cell_states.insert(key, state);
        }
    }

    /// The whole unified per-cell state map (save codec, wire payload, light
    /// snapshot, mesh-pad capture).
    #[inline]
    pub(crate) fn cell_states(&self) -> &HashMap<u16, ShapeState> {
        match &self.sparse {
            Some(s) => &s.cell_states,
            None => empty_map!(ShapeState),
        }
    }

    pub(crate) fn cell_kv_get(&self, x: usize, y: usize, z: usize, key: &str) -> Option<&[u8]> {
        self.cell_kv()
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
        self.sparse_mut()
            .cell_kv
            .entry(Self::key(x, y, z))
            .or_default()
            .insert(key, value);
    }

    pub(crate) fn cell_kv_remove(&mut self, x: usize, y: usize, z: usize, key: &str) -> bool {
        let idx = Self::key(x, y, z);
        let Some(s) = self.sparse.as_deref_mut() else {
            return false;
        };
        let Some(map) = s.cell_kv.get_mut(&idx) else {
            return false;
        };
        let removed = map.remove(key).is_some();
        if map.is_empty() {
            s.cell_kv.remove(&idx);
        }
        removed
    }

    #[inline]
    pub(crate) fn cell_kv(&self) -> &HashMap<u16, BTreeMap<String, Vec<u8>>> {
        match &self.sparse {
            Some(s) => &s.cell_kv,
            None => empty_map!(BTreeMap<String, Vec<u8>>),
        }
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
        self.sparse
            .as_deref_mut()?
            .cell_kv
            .remove(&Self::key(x, y, z))
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
            self.sparse_mut().cell_kv.insert(Self::key(x, y, z), map);
        }
    }
}
