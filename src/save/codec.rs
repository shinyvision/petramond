//! Binary codec for save data: little-endian primitives + compressed section
//! records.
//!
//! A section record stores only what generation can't reproduce for one 16³ cube —
//! block ids and (when present) water-flow metadata — then zlib-compresses the lot
//! (flate2 / miniz_oxide, pure Rust). Biome and surface heightmap are per-column,
//! cheaply regenerated, and so are never written here; skylight is recomputed on
//! load.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};

use crate::block_state::{LogAxis, SlabState, StairState};
use crate::chunk::{SectionPos, SECTION_VOLUME};
use crate::door::DoorState;
use crate::entity::DroppedItem;
use crate::furnace::{Facing, Furnace};
use crate::item::{ItemStack, ItemType};
use crate::mob::SavedMob;
use crate::container::Container;
use crate::section::Section;
use crate::torch::TorchPlacement;

/// Current section-record version. Flag-gated payloads are appended at the end, so a
/// new one that fits a free flag bit needs no version bump. The cubic format starts
/// fresh at `1` (the column-era chunk records are not migrated — saves regenerate).
/// v2 widens the per-mob record with the shear-regrow counter (see `save::mobs`);
/// v3 widens it again with the per-mob mod KV map (default-empty for older records).
/// v4 appends a third flags byte for slab layer state.
/// v5 unifies all slot storage into one generic container list (chests,
/// furnaces, mod containers), splits the furnace record into pure machine
/// state, and adds the shared entity-facing list. v5 is a CLEAN BREAK
/// (`SECTION_REC_MIN_VERSION` = 5): pre-v5 records do not load — the game is
/// unreleased, dev worlds regenerate (see WIKI/project-rules.md).
const SECTION_REC_VERSION: u8 = 5;
/// Oldest section-record version this build can still read.
const SECTION_REC_MIN_VERSION: u8 = 5;
const FLAG_HAS_WATER: u8 = 0x01;
const FLAG_HAS_ENTITIES: u8 = 0x02;
const FLAG_HAS_FURNACES: u8 = 0x04;
const FLAG_HAS_ENTITY_FACINGS: u8 = 0x08;
const FLAG_HAS_TORCHES: u8 = 0x10;
const FLAG_HAS_MOBS: u8 = 0x20;
const FLAG_HAS_MODEL_CELLS: u8 = 0x40;
const FLAG_HAS_MODEL_FACINGS: u8 = 0x80;
/// Second flags byte (chunk-record v3+). `0` for a v2 record (no such byte).
const FLAG2_HAS_SAPLINGS: u8 = 0x01;
const FLAG2_HAS_DOORS: u8 = 0x02;
const FLAG2_HAS_STAIRS: u8 = 0x04;
const FLAG2_HAS_CELL_KV: u8 = 0x08;
const FLAG2_HAS_LOG_AXES: u8 = 0x10;
const FLAG2_HAS_CONTAINERS: u8 = 0x20;
/// Third flags byte (section-record v4+). `0` for older records.
const FLAG3_HAS_SLABS: u8 = 0x01;

/// Owned, send-able copy of one 16³ section's save data. The game thread builds one
/// of these (a cheap array clone) and hands it to the I/O thread, which does the
/// expensive compression off the game loop. Biome/heightmap are per-column and
/// regenerated, so they are not part of a section record.
pub struct SectionSnapshot {
    pub pos: SectionPos,
    pub blocks: Box<[u8]>,
    pub water: Option<Box<[u8]>>,
    /// Item entities resting in this section, captured at save time so their
    /// lifetime timers persist with it. Empty for the common case.
    pub entities: Vec<DroppedItem>,
    /// Furnace machine state (burn/cook counters) in this section, keyed by
    /// section-local block index. The slots live in [`containers`](Self::containers).
    /// Empty for the common section.
    pub furnaces: HashMap<u16, Furnace>,
    /// Generic item-slot containers (chests, furnaces, mod container blocks),
    /// keyed by section-local block index. Empty for the common section.
    pub containers: HashMap<u16, Container>,
    /// Facing block-entity orientations (chests, furnaces), keyed by
    /// section-local block index. Empty for the common section.
    pub entity_facings: HashMap<u16, Facing>,
    /// Torch orientations in this section, keyed by section-local block index, so a
    /// wall vs floor torch reloads the way it was placed. Empty for the common section.
    pub torches: HashMap<u16, TorchPlacement>,
    /// Multi-cell bbmodel occupancy: each non-zero authored footprint offset, so a
    /// placed multi-block (the workbench) reloads as one object. Empty for the common
    /// section. See `Section::model_cells`.
    pub model_cells: HashMap<u16, [u8; 3]>,
    /// Per-cell facing for oriented bbmodel blocks, keyed like `model_cells`. Empty for
    /// old/non-directional model placements. See `Section::model_facings`.
    pub model_facings: HashMap<u16, Facing>,
    /// Sapling growth stages (`0..=2`) in this section, keyed by section-local index,
    /// so a half-grown sapling reloads at the stage it reached. Empty for the common
    /// section. See `Section::sapling_stages`.
    pub sapling_stages: HashMap<u16, u8>,
    /// Door state (facing + open + which-half), keyed by section-local index, so a
    /// placed door reloads on the same edge and in the same open/closed pose. Empty for
    /// the common section. See `Section::doors` / [`crate::door`].
    pub doors: HashMap<u16, DoorState>,
    /// State of placed stairs, keyed by section-local index, so a stair reloads with
    /// the same low/open side and top/bottom half.
    pub stair_states: HashMap<u16, StairState>,
    /// State of placed slabs, keyed by section-local index. Stores split axis plus
    /// the material block in each occupied half so mixed slab stacks reload exactly.
    pub slab_states: HashMap<u16, SlabState>,
    /// Non-default log axes, keyed by section-local index. Missing logs are vertical.
    pub log_axes: HashMap<u16, LogAxis>,
    /// Per-cell mod KV entries (`mod_id:key` → bytes), keyed by section-local
    /// index. Opaque to the engine and PRESERVED byte-exact through load/save —
    /// unknown keys are never dropped, so an absent mod's data survives. See
    /// `Section::cell_kv`.
    pub cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
    /// Mobs resting in this section, captured at save time so a passive owl reloads
    /// where it was left. Like [`entities`](Self::entities) these don't live in the
    /// `Section`, so the world save paths set this from the live mob set. Empty for the
    /// common section.
    pub mobs: Vec<SavedMob>,
}

impl SectionSnapshot {
    /// Snapshot a section's terrain with no entities or mobs attached. The world save
    /// paths set [`entities`](Self::entities) / [`mobs`](Self::mobs) afterwards from the
    /// active item and mob sets.
    pub fn from_section(s: &Section) -> Self {
        Self {
            pos: SectionPos::new(s.cx, s.cy, s.cz),
            blocks: Box::from(s.blocks_slice()),
            water: s.water_slice().map(Box::from),
            entities: Vec::new(),
            furnaces: s.furnaces().clone(),
            containers: s.containers().clone(),
            entity_facings: s.entity_facings().clone(),
            torches: s.torches().clone(),
            model_cells: s.model_cells().clone(),
            model_facings: s.model_facings().clone(),
            sapling_stages: s.sapling_stages().clone(),
            doors: s.doors().clone(),
            stair_states: s.stair_states().clone(),
            slab_states: s.slab_states().clone(),
            log_axes: s.log_axes().clone(),
            cell_kv: s.cell_kv().clone(),
            mobs: Vec::new(),
        }
    }
}

/// Sequential little-endian reader. Every read is bounds-checked and returns
/// `None` past the end, so a truncated / corrupt file fails cleanly.
pub struct Reader<'a> {
    bytes: &'a [u8],
    off: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, off: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.off.checked_add(n)?;
        let s = self.bytes.get(self.off..end)?;
        self.off = end;
        Some(s)
    }
    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.arr()?))
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        self.take(n)
    }
}

pub(crate) fn put_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

pub(crate) fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// Encode one inventory/container slot as `[item id, count]`, with `[0, 0]` for an
/// empty or absent slot. Shared by the `level` (inventory/cursor) and `furnace`
/// codecs so the 2-byte slot format lives in exactly one place.
pub fn put_item_slot(buf: &mut Vec<u8>, slot: Option<ItemStack>) {
    match slot {
        Some(s) if !s.is_empty() => {
            put_u8(buf, super::palette::active().item_to_disk(s.item.id()));
            put_u8(buf, s.count);
        }
        _ => {
            put_u8(buf, 0);
            put_u8(buf, 0);
        }
    }
}

/// Decode a slot written by [`put_item_slot`]: `None` on truncated input,
/// `Some(None)` for an empty slot, else the stack.
pub fn get_item_slot(r: &mut Reader) -> Option<Option<ItemStack>> {
    let id = r.u8()?;
    let count = r.u8()?;
    if id == 0 || count == 0 {
        Some(None)
    } else {
        let id = super::palette::active().item_from_disk(id);
        Some(Some(ItemStack::new(ItemType::from_id(id), count)))
    }
}

/// Append a `u16`-length-prefixed list of `(local index, record)` entries to
/// `buf`, in ascending index order so identical state encodes identically. Owns
/// only the list FRAME — the count (capped at `u16::MAX`, since a chunk never
/// holds anywhere near that many), the sort-by-index reproducibility invariant,
/// the `2 + n * rec_bytes` reserve, and the per-entry `u16` index — and defers
/// the record body to `body`. Shared by the furnace and chest codecs.
pub(crate) fn put_indexed<T>(
    buf: &mut Vec<u8>,
    map: &HashMap<u16, T>,
    rec_bytes: usize,
    mut body: impl FnMut(&mut Vec<u8>, &T),
) {
    let n = map.len().min(u16::MAX as usize);
    buf.reserve(2 + n * rec_bytes);
    put_u16(buf, n as u16);
    let mut entries: Vec<(&u16, &T)> = map.iter().take(n).collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (idx, rec) in entries {
        put_u16(buf, *idx);
        body(buf, rec);
    }
}

/// Read an indexed list written by [`put_indexed`]: the `u16` count, then each
/// `u16` index paired with a record decoded by `body`. `None` on truncated
/// input (propagated from either the index read or `body`).
pub(crate) fn get_indexed<T>(
    r: &mut Reader,
    mut body: impl FnMut(&mut Reader) -> Option<T>,
) -> Option<HashMap<u16, T>> {
    let n = r.u16()? as usize;
    let mut out = HashMap::with_capacity(n.min(256));
    for _ in 0..n {
        let idx = r.u16()?;
        out.insert(idx, body(r)?);
    }
    Some(out)
}

/// Append a mod KV map: `u16` entry count, then per entry a `u16`-length-
/// prefixed key + `u32`-length-prefixed value. BTreeMap iteration is sorted,
/// so identical maps encode identically (the determinism the byte-exact
/// preservation contract rests on). An entry with an oversized key (> u16 —
/// the HostCall boundary caps keys far below this) is skipped defensively.
/// Shared by the per-cell (section) and per-mob KV payloads.
pub(crate) fn put_kv_map(buf: &mut Vec<u8>, map: &BTreeMap<String, Vec<u8>>) {
    let entries: Vec<(&String, &Vec<u8>)> = map
        .iter()
        .filter(|(k, _)| k.len() <= u16::MAX as usize)
        .take(u16::MAX as usize)
        .collect();
    put_u16(buf, entries.len() as u16);
    for (k, v) in entries {
        put_u16(buf, k.len() as u16);
        buf.extend_from_slice(k.as_bytes());
        put_u32(buf, v.len() as u32);
        buf.extend_from_slice(v);
    }
}

/// Read a mod KV map written by [`put_kv_map`]; `None` on truncated or
/// non-UTF-8 input.
pub(crate) fn get_kv_map(r: &mut Reader) -> Option<BTreeMap<String, Vec<u8>>> {
    let n = r.u16()? as usize;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let klen = r.u16()? as usize;
        let key = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let vlen = r.u32()? as usize;
        out.insert(key, r.bytes(vlen)?.to_vec());
    }
    Some(out)
}

/// zlib-compress a payload.
pub fn deflate(payload: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = e.write_all(payload);
    e.finish().unwrap_or_default()
}

/// zlib-decompress; `None` on corrupt input.
pub fn inflate(blob: &[u8]) -> Option<Vec<u8>> {
    let mut d = flate2::read::ZlibDecoder::new(blob);
    let mut out = Vec::new();
    d.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Compress a section snapshot into a record: `[version, flags, flags2, flags3, blocks,
/// water?, entities?, …]`, zlib-deflated. Each flag-gated payload is appended only
/// when present, so a terrain-only section pays for just its block array.
pub fn encode_snapshot(s: &SectionSnapshot) -> Vec<u8> {
    let extra = s.water.as_ref().map_or(0, |w| w.len());
    let mut payload = Vec::with_capacity(4 + s.blocks.len() + extra);
    put_u8(&mut payload, SECTION_REC_VERSION);
    let mut flags = 0u8;
    if s.water.is_some() {
        flags |= FLAG_HAS_WATER;
    }
    if !s.entities.is_empty() {
        flags |= FLAG_HAS_ENTITIES;
    }
    if !s.furnaces.is_empty() {
        flags |= FLAG_HAS_FURNACES;
    }
    if !s.entity_facings.is_empty() {
        flags |= FLAG_HAS_ENTITY_FACINGS;
    }
    if !s.torches.is_empty() {
        flags |= FLAG_HAS_TORCHES;
    }
    if !s.mobs.is_empty() {
        flags |= FLAG_HAS_MOBS;
    }
    if !s.model_cells.is_empty() {
        flags |= FLAG_HAS_MODEL_CELLS;
    }
    if !s.model_facings.is_empty() {
        flags |= FLAG_HAS_MODEL_FACINGS;
    }
    let mut flags2 = 0u8;
    if !s.sapling_stages.is_empty() {
        flags2 |= FLAG2_HAS_SAPLINGS;
    }
    if !s.doors.is_empty() {
        flags2 |= FLAG2_HAS_DOORS;
    }
    if !s.stair_states.is_empty() {
        flags2 |= FLAG2_HAS_STAIRS;
    }
    if !s.cell_kv.is_empty() {
        flags2 |= FLAG2_HAS_CELL_KV;
    }
    if !s.log_axes.is_empty() {
        flags2 |= FLAG2_HAS_LOG_AXES;
    }
    if !s.containers.is_empty() {
        flags2 |= FLAG2_HAS_CONTAINERS;
    }
    let mut flags3 = 0u8;
    if !s.slab_states.is_empty() {
        flags3 |= FLAG3_HAS_SLABS;
    }
    put_u8(&mut payload, flags);
    put_u8(&mut payload, flags2);
    put_u8(&mut payload, flags3);
    // Block ids are stored as the SAVE's ids (see `super::palette`), so a
    // future registry renumbering can't corrupt old worlds.
    let pal = super::palette::active();
    payload.extend(s.blocks.iter().map(|&b| pal.block_to_disk(b)));
    if let Some(w) = &s.water {
        payload.extend_from_slice(w);
    }
    if !s.entities.is_empty() {
        super::entities::put_entities(&mut payload, &s.entities);
    }
    if !s.furnaces.is_empty() {
        super::furnace::put_furnaces(&mut payload, &s.furnaces);
    }
    if !s.entity_facings.is_empty() {
        put_indexed(&mut payload, &s.entity_facings, 1, |buf, facing| {
            put_u8(buf, facing.to_u8());
        });
    }
    if !s.torches.is_empty() {
        super::torch::put_torches(&mut payload, &s.torches);
    }
    if !s.mobs.is_empty() {
        super::mobs::put_mobs(&mut payload, &s.mobs);
    }
    if !s.model_cells.is_empty() {
        // Each record is the cell's 3-byte footprint offset (idx written by put_indexed).
        put_indexed(&mut payload, &s.model_cells, 3, |buf, off| {
            put_u8(buf, off[0]);
            put_u8(buf, off[1]);
            put_u8(buf, off[2]);
        });
    }
    if !s.model_facings.is_empty() {
        put_indexed(&mut payload, &s.model_facings, 1, |buf, facing| {
            put_u8(buf, facing.to_u8());
        });
    }
    if !s.sapling_stages.is_empty() {
        // Each record is the cell's 1-byte growth stage (idx written by put_indexed).
        put_indexed(&mut payload, &s.sapling_stages, 1, |buf, stage| {
            put_u8(buf, *stage);
        });
    }
    if !s.doors.is_empty() {
        // Each record is the cell's 1-byte packed door state (idx written by put_indexed).
        put_indexed(&mut payload, &s.doors, 1, |buf, state| {
            put_u8(buf, state.encode());
        });
    }
    if !s.stair_states.is_empty() {
        put_indexed(&mut payload, &s.stair_states, 1, |buf, state| {
            put_u8(buf, state.encode());
        });
    }
    if !s.cell_kv.is_empty() {
        // Each record is the cell's KV map (idx written by put_indexed);
        // rec_bytes is a reserve hint only — the record body is variable.
        put_indexed(&mut payload, &s.cell_kv, 16, |buf, map| {
            put_kv_map(buf, map);
        });
    }
    if !s.log_axes.is_empty() {
        put_indexed(&mut payload, &s.log_axes, 1, |buf, axis| {
            put_u8(buf, axis.to_u8());
        });
    }
    if !s.containers.is_empty() {
        super::container::put_containers(&mut payload, &s.containers);
    }
    if !s.slab_states.is_empty() {
        put_indexed(&mut payload, &s.slab_states, 3, |buf, state| {
            put_u8(buf, state.encode_meta());
            put_u8(buf, pal.block_to_disk(state.layers[0].id()));
            put_u8(buf, pal.block_to_disk(state.layers[1].id()));
        });
    }
    deflate(&payload)
}

/// Decode a compressed section record into a `Section` at `pos` plus any item
/// entities and mobs stored with it. `None` on corrupt / wrong-version /
/// wrong-length data.
pub fn decode_section(
    pos: SectionPos,
    blob: &[u8],
) -> Option<(Section, Vec<DroppedItem>, Vec<SavedMob>)> {
    let payload = inflate(blob)?;
    let mut r = Reader::new(&payload);
    let version = r.u8()?;
    if !(SECTION_REC_MIN_VERSION..=SECTION_REC_VERSION).contains(&version) {
        return None;
    }
    let flags = r.u8()?;
    let flags2 = r.u8()?;
    let flags3 = r.u8()?;
    let pal = super::palette::active();
    let blocks: Box<[u8]> = r
        .bytes(SECTION_VOLUME)?
        .iter()
        .map(|&b| pal.block_from_disk(b))
        .collect();
    let water = if flags & FLAG_HAS_WATER != 0 {
        Some(r.bytes(SECTION_VOLUME)?.to_vec().into_boxed_slice())
    } else {
        None
    };
    let entities = if flags & FLAG_HAS_ENTITIES != 0 {
        super::entities::get_entities(&mut r)?
    } else {
        Vec::new()
    };
    let furnaces = if flags & FLAG_HAS_FURNACES != 0 {
        super::furnace::get_furnaces(&mut r)?
    } else {
        HashMap::new()
    };
    let entity_facings = if flags & FLAG_HAS_ENTITY_FACINGS != 0 {
        get_indexed(&mut r, |r| Some(Facing::from_u8(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let torches = if flags & FLAG_HAS_TORCHES != 0 {
        super::torch::get_torches(&mut r)?
    } else {
        HashMap::new()
    };
    let mobs = if flags & FLAG_HAS_MOBS != 0 {
        super::mobs::get_mobs(&mut r, version)?
    } else {
        Vec::new()
    };
    let model_cells = if flags & FLAG_HAS_MODEL_CELLS != 0 {
        get_indexed(&mut r, |r| Some([r.u8()?, r.u8()?, r.u8()?]))?
    } else {
        HashMap::new()
    };
    let model_facings = if flags & FLAG_HAS_MODEL_FACINGS != 0 {
        get_indexed(&mut r, |r| Some(Facing::from_u8(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let sapling_stages = if flags2 & FLAG2_HAS_SAPLINGS != 0 {
        get_indexed(&mut r, |r| r.u8())?
    } else {
        HashMap::new()
    };
    let doors = if flags2 & FLAG2_HAS_DOORS != 0 {
        get_indexed(&mut r, |r| Some(DoorState::decode(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let stair_states = if flags2 & FLAG2_HAS_STAIRS != 0 {
        get_indexed(&mut r, |r| Some(StairState::decode(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let cell_kv = if flags2 & FLAG2_HAS_CELL_KV != 0 {
        get_indexed(&mut r, get_kv_map)?
    } else {
        HashMap::new()
    };
    let log_axes = if flags2 & FLAG2_HAS_LOG_AXES != 0 {
        get_indexed(&mut r, |r| Some(LogAxis::from_u8(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let containers = if flags2 & FLAG2_HAS_CONTAINERS != 0 {
        super::container::get_containers(&mut r)?
    } else {
        HashMap::new()
    };
    let slab_states = if flags3 & FLAG3_HAS_SLABS != 0 {
        get_indexed(&mut r, |r| {
            let meta = r.u8()?;
            let a = crate::block::Block(pal.block_from_disk(r.u8()?));
            let b = crate::block::Block(pal.block_from_disk(r.u8()?));
            Some(SlabState::decode(meta, a, b))
        })?
    } else {
        HashMap::new()
    };
    Some((
        Section::from_saved(
            pos.cx,
            pos.cy,
            pos.cz,
            blocks,
            water,
            furnaces,
            containers,
            entity_facings,
            torches,
            model_cells,
            model_facings,
            sapling_stages,
            doors,
            stair_states,
            slab_states,
            log_axes,
            cell_kv,
        ),
        entities,
        mobs,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::block_state::SlabSplit;
    use crate::mathh::Vec3;

    fn sec(cx: i32, cy: i32, cz: i32) -> Section {
        Section::new(cx, cy, cz)
    }

    #[test]
    fn section_record_roundtrips() {
        // A section spans world Y [cy*16 .. cy*16+16); negative cy is in range.
        let mut s = sec(-3, -2, 7);
        s.set_block(1, 4, 2, Block::Stone);
        s.set_block(0, 10, 0, Block::Grass);
        s.set_water(5, 5, 5, Block::Water, 0x23);

        let snap = SectionSnapshot::from_section(&s);
        let blob = encode_snapshot(&snap);
        let (back, entities, mobs) =
            decode_section(SectionPos::new(-3, -2, 7), &blob).expect("decodes");

        assert_eq!((back.cx, back.cy, back.cz), (-3, -2, 7));
        assert_eq!(back.block_raw(1, 4, 2), Block::Stone.id());
        assert_eq!(back.block_raw(0, 10, 0), Block::Grass.id());
        assert_eq!(back.block_raw(5, 5, 5), Block::Water.id());
        assert_eq!(back.water_meta(5, 5, 5), 0x23);
        assert!(!back.modified);
        assert!(entities.is_empty(), "no entities attached");
        assert!(mobs.is_empty(), "no mobs attached");
    }

    #[test]
    fn section_record_roundtrips_entities() {
        let mut s = sec(2, 4, 2);
        s.set_block(8, 0, 8, Block::Dirt);
        let mut snap = SectionSnapshot::from_section(&s);
        let mut drop = DroppedItem::new(
            Vec3::new(40.5, 65.0, 40.5),
            ItemStack::new(ItemType::Stone, 7),
            1,
        );
        drop.ticks_lived = 1234;
        snap.entities.push(drop);

        let blob = encode_snapshot(&snap);
        let (_back, entities, _mobs) =
            decode_section(SectionPos::new(2, 4, 2), &blob).expect("decodes");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].stack, ItemStack::new(ItemType::Stone, 7));
        assert_eq!(
            entities[0].ticks_lived, 1234,
            "remaining lifetime survives the save"
        );
    }

    #[test]
    fn section_record_roundtrips_mobs() {
        let mut s = sec(-1, 4, 4);
        s.set_block(8, 0, 8, Block::Dirt);
        let mut snap = SectionSnapshot::from_section(&s);
        snap.mobs.push(SavedMob {
            kind: crate::mob::Mob::Owl,
            pos: Vec3::new(-12.5, 65.0, 72.25),
            yaw: 1.75,
            shear_regrow: 0,
            kv: Default::default(),
        });

        let blob = encode_snapshot(&snap);
        let (_back, _entities, mobs) =
            decode_section(SectionPos::new(-1, 4, 4), &blob).expect("decodes");
        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].kind, crate::mob::Mob::Owl);
        assert_eq!(
            mobs[0].pos,
            Vec3::new(-12.5, 65.0, 72.25),
            "position persists"
        );
        assert_eq!(mobs[0].yaw, 1.75, "facing persists");
    }

    #[test]
    fn section_record_roundtrips_furnaces() {
        use crate::furnace::{FURNACE_SLOTS, SLOT_FUEL, SLOT_INPUT};
        let mut s = sec(1, 4, 1);
        s.set_block(2, 1, 3, Block::Furnace);
        s.insert_furnace(
            2,
            1,
            3,
            crate::furnace::Furnace {
                cook_progress: 200,
                burn_remaining: 1000,
                burn_max: 4800,
            },
        );
        let mut container = crate::container::Container::with_len(FURNACE_SLOTS);
        container.slots[SLOT_INPUT] = Some(ItemStack::new(ItemType::RawCopper, 12));
        container.slots[SLOT_FUEL] = Some(ItemStack::new(ItemType::Coal, 1));
        s.insert_container(2, 1, 3, container);
        s.insert_entity_facing(2, 1, 3, crate::furnace::Facing::West);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(1, 4, 1), &blob).expect("decodes");

        assert_eq!(back.block_raw(2, 1, 3), Block::Furnace.id());
        let f = back.furnace_at(2, 1, 3).expect("furnace restored");
        assert_eq!(f.cook_progress, 200);
        assert_eq!(f.burn_remaining, 1000);
        assert!(f.is_lit(), "a saved burning furnace reloads lit");
        let c = back.container_at(2, 1, 3).expect("slots restored");
        assert_eq!(c.slots[SLOT_INPUT], Some(ItemStack::new(ItemType::RawCopper, 12)));
        assert_eq!(c.slots[SLOT_FUEL], Some(ItemStack::new(ItemType::Coal, 1)));
        assert_eq!(
            back.entity_facing(2, 1, 3),
            crate::furnace::Facing::West,
            "facing persists"
        );
    }

    #[test]
    fn section_record_roundtrips_chests() {
        let mut s = sec(4, 4, -2);
        s.set_block(9, 2, 1, Block::Chest);
        let mut chest =
            crate::container::Container::with_len(crate::world::chest::CHEST_SLOTS);
        chest.slots[0] = Some(ItemStack::new(ItemType::Stone, 64));
        chest.slots[26] = Some(ItemStack::new(ItemType::OakLog, 5));
        s.insert_container(9, 2, 1, chest);
        s.insert_entity_facing(9, 2, 1, crate::furnace::Facing::South);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(4, 4, -2), &blob).expect("decodes");

        assert_eq!(back.block_raw(9, 2, 1), Block::Chest.id());
        let got = back.container_at(9, 2, 1).expect("chest restored");
        assert_eq!(got.slots[0], Some(ItemStack::new(ItemType::Stone, 64)));
        assert_eq!(got.slots[26], Some(ItemStack::new(ItemType::OakLog, 5)));
        assert_eq!(got.slots[5], None);
        assert_eq!(
            back.entity_facing(9, 2, 1),
            crate::furnace::Facing::South,
            "facing persists"
        );
    }

    #[test]
    fn section_record_roundtrips_torches() {
        use crate::torch::TorchPlacement;
        let mut s = sec(6, 4, 6);
        s.set_block(3, 3, 4, Block::Torch);
        s.insert_torch(3, 3, 4, TorchPlacement::East);
        s.set_block(3, 4, 4, Block::Torch);
        s.insert_torch(3, 4, 4, TorchPlacement::Floor);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(6, 4, 6), &blob).expect("decodes");

        assert_eq!(back.block_raw(3, 3, 4), Block::Torch.id());
        assert_eq!(
            back.torch_placement(3, 3, 4),
            TorchPlacement::East,
            "wall mount persists"
        );
        assert_eq!(
            back.torch_placement(3, 4, 4),
            TorchPlacement::Floor,
            "floor mount persists"
        );
        // A cell with no torch reads the Floor default.
        assert_eq!(back.torch_placement(0, 0, 0), TorchPlacement::Floor);
    }

    #[test]
    fn section_record_roundtrips_model_cells() {
        // A placed multi-block records authored footprint offsets and per-cell facing;
        // both must survive a save/load so the block reloads as one object.
        let mut s = sec(2, 4, 3);
        s.set_block(5, 0, 5, Block::FurnitureWorkbench);
        s.set_block(6, 0, 5, Block::FurnitureWorkbench);
        s.set_model_offset(6, 0, 5, [1, 0, 0]);
        s.set_model_facing(6, 0, 5, Facing::East);
        s.set_block(5, 1, 5, Block::FurnitureWorkbench);
        s.set_model_offset(5, 1, 5, [0, 1, 0]);
        s.set_model_facing(5, 1, 5, Facing::East);
        s.set_model_facing(5, 0, 5, Facing::East);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(2, 4, 3), &blob).expect("decodes");

        assert_eq!(back.block_raw(6, 0, 5), Block::FurnitureWorkbench.id());
        assert_eq!(back.model_offset(6, 0, 5), [1, 0, 0], "x-offset persists");
        assert_eq!(back.model_offset(5, 1, 5), [0, 1, 0], "y-offset persists");
        assert_eq!(back.model_facing(6, 0, 5), Facing::East, "facing persists");
        assert_eq!(back.model_facing(5, 1, 5), Facing::East, "facing persists");
        assert_eq!(
            back.model_facing(5, 0, 5),
            Facing::East,
            "origin facing persists"
        );
        // The origin cell stores no offset and reads the [0,0,0] default.
        assert_eq!(back.model_offset(5, 0, 5), [0, 0, 0]);
    }

    #[test]
    fn section_record_roundtrips_sapling_stages() {
        // A half-grown sapling must reload at the stage it reached. The stage is set
        // AFTER the block (set_block clears it).
        let mut s = sec(5, 4, -1);
        s.set_block(2, 0, 3, Block::OakSapling);
        s.set_sapling_stage(2, 0, 3, 2);
        s.set_block(7, 6, 1, Block::BirchSapling);
        s.set_sapling_stage(7, 6, 1, 1);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(5, 4, -1), &blob).expect("decodes");

        assert_eq!(back.block_raw(2, 0, 3), Block::OakSapling.id());
        assert_eq!(back.sapling_stage(2, 0, 3), 2, "oak stage persists");
        assert_eq!(back.sapling_stage(7, 6, 1), 1, "birch stage persists");
        // A cell with no recorded stage reads 0.
        assert_eq!(back.sapling_stage(0, 0, 0), 0);
    }

    #[test]
    fn section_record_roundtrips_doors() {
        // A placed door's facing + open + which-half state must reload exactly. State is
        // set AFTER the block.
        use crate::door::DoorState;
        use crate::furnace::Facing;
        let mut s = sec(3, 4, 7);
        s.set_block(4, 0, 5, Block::OakDoor);
        s.set_door_state(
            4,
            0,
            5,
            DoorState {
                facing: Facing::East,
                open: true,
                top: false,
            },
        );
        s.set_block(4, 1, 5, Block::OakDoor);
        s.set_door_state(
            4,
            1,
            5,
            DoorState {
                facing: Facing::East,
                open: true,
                top: true,
            },
        );

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(3, 4, 7), &blob).expect("decodes");

        assert_eq!(back.block_raw(4, 0, 5), Block::OakDoor.id());
        assert_eq!(
            back.door_state(4, 0, 5),
            Some(DoorState {
                facing: Facing::East,
                open: true,
                top: false
            })
        );
        assert_eq!(
            back.door_state(4, 1, 5).map(|s| s.top),
            Some(true),
            "the upper half persists its top bit"
        );
        // A non-door cell carries no door state.
        assert_eq!(back.door_state(0, 0, 0), None);
    }

    #[test]
    fn section_record_roundtrips_stair_states() {
        let mut s = sec(7, 4, 1);
        s.set_block(2, 0, 3, Block::OakStairs);
        s.set_stair_facing(2, 0, 3, Facing::West);
        s.set_block(9, 5, 1, Block::StoneStairs);
        s.set_stair_state(
            9,
            5,
            1,
            StairState::new(Facing::South, crate::block_state::StairHalf::Top),
        );

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(7, 4, 1), &blob).expect("decodes");

        assert_eq!(back.block_raw(2, 0, 3), Block::OakStairs.id());
        assert_eq!(back.stair_facing(2, 0, 3), Facing::West);
        assert_eq!(back.block_raw(9, 5, 1), Block::StoneStairs.id());
        assert_eq!(
            back.stair_state(9, 5, 1),
            StairState::new(Facing::South, crate::block_state::StairHalf::Top)
        );
        assert_eq!(back.stair_state(0, 0, 0), StairState::default());
    }

    #[test]
    fn section_record_roundtrips_slab_states() {
        let mut s = sec(7, 4, 2);
        let state = SlabState {
            split: SlabSplit::Y,
            layers: [Block::DirtSlab, Block::CobblestoneSlab],
        };
        s.set_block(4, 2, 4, Block::CobblestoneSlab);
        s.set_slab_state(4, 2, 4, state);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(7, 4, 2), &blob).expect("decodes");

        assert_eq!(back.block_raw(4, 2, 4), Block::CobblestoneSlab.id());
        assert_eq!(back.slab_state(4, 2, 4), state);
        assert_eq!(back.slab_state(0, 0, 0), SlabState::EMPTY);
    }

    #[test]
    fn section_record_roundtrips_log_axes() {
        let mut s = sec(7, 4, 1);
        s.set_block(2, 0, 3, Block::OakLog);
        s.set_log_axis(2, 0, 3, LogAxis::X);
        s.set_block(9, 5, 1, Block::SpruceLog);
        s.set_log_axis(9, 5, 1, LogAxis::Z);

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(7, 4, 1), &blob).expect("decodes");

        assert_eq!(back.log_axis(2, 0, 3), LogAxis::X);
        assert_eq!(back.log_axis(9, 5, 1), LogAxis::Z);
        assert_eq!(back.log_axis(0, 0, 0), LogAxis::Y);
    }

    #[test]
    fn section_record_roundtrips_cell_kv() {
        let mut s = sec(1, 4, 1);
        s.set_block(2, 3, 4, Block::Stone);
        s.cell_kv_set(2, 3, 4, "farm:moisture".into(), vec![7]);
        s.cell_kv_set(0, 0, 0, "othermod:tag".into(), Vec::new());

        let blob = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (back, _entities, _mobs) =
            decode_section(SectionPos::new(1, 4, 1), &blob).expect("decodes");

        assert_eq!(back.cell_kv_get(2, 3, 4, "farm:moisture"), Some(&[7u8][..]));
        assert_eq!(
            back.cell_kv_get(0, 0, 0, "othermod:tag"),
            Some(&[][..]),
            "empty values are values"
        );
        assert_eq!(back.cell_kv_get(2, 3, 4, "farm:missing"), None);
        assert_eq!(back.cell_kv_get(9, 9, 9, "farm:moisture"), None);
    }

    /// The preservation contract: a record carrying cell KV nobody reads
    /// (the owning mod is absent) must survive a load → save cycle BYTE-EXACT —
    /// unknown keys are never dropped and the encoding is deterministic.
    #[test]
    fn cell_kv_is_preserved_byte_exact_through_load_and_save() {
        let mut s = sec(0, 4, 0);
        s.set_block(1, 1, 1, Block::Dirt);
        s.cell_kv_set(1, 1, 1, "ghostmod:data".into(), vec![1, 2, 3, 4]);
        s.cell_kv_set(1, 1, 1, "ghostmod:aaa".into(), vec![5]);
        s.cell_kv_set(5, 5, 5, "ghostmod:other".into(), vec![9]);

        let blob1 = encode_snapshot(&SectionSnapshot::from_section(&s));
        let (loaded, _, _) = decode_section(SectionPos::new(0, 4, 0), &blob1).expect("decodes");
        let blob2 = encode_snapshot(&SectionSnapshot::from_section(&loaded));
        assert_eq!(blob1, blob2, "an untouched record re-encodes byte-exact");
    }

    /// The stale-record guard: once the last entry is removed the has-cell-kv
    /// flag clears, so a re-saved record is indistinguishable from one that
    /// never carried KV — nothing lingers to resurrect.
    #[test]
    fn emptied_cell_kv_clears_its_record_flag() {
        let clean = {
            let mut s = sec(2, 4, 2);
            s.set_block(3, 3, 3, Block::Stone);
            encode_snapshot(&SectionSnapshot::from_section(&s))
        };
        let mut s = sec(2, 4, 2);
        s.set_block(3, 3, 3, Block::Stone);
        s.cell_kv_set(3, 3, 3, "farm:moisture".into(), vec![1]);
        assert_ne!(
            encode_snapshot(&SectionSnapshot::from_section(&s)),
            clean,
            "the tagged record differs"
        );
        assert!(s.cell_kv_remove(3, 3, 3, "farm:moisture"));
        assert_eq!(
            encode_snapshot(&SectionSnapshot::from_section(&s)),
            clean,
            "removing the last entry restores the untagged encoding"
        );
    }

    #[test]
    fn water_free_section_omits_water() {
        let mut s = sec(0, 4, 0);
        s.set_block(8, 0, 8, Block::Dirt);
        let snap = SectionSnapshot::from_section(&s);
        assert!(snap.water.is_none());
        let blob = encode_snapshot(&snap);
        let (back, _, _) = decode_section(SectionPos::new(0, 4, 0), &blob).expect("decodes");
        assert_eq!(back.water_meta(8, 0, 8), 0);
    }

    #[test]
    fn corrupt_blob_is_none() {
        let p = SectionPos::new(0, 0, 0);
        assert!(decode_section(p, &[1, 2, 3, 4]).is_none());
        assert!(decode_section(p, &[]).is_none());
    }
}
