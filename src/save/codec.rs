//! Binary codec for save data: little-endian primitives + compressed chunk
//! records.
//!
//! A chunk record stores only what generation can't reproduce — block ids,
//! biome ids, and (when present) water-flow metadata — then zlib-compresses the
//! lot (flate2 / miniz_oxide, pure Rust). Heightmap and skylight are recomputed
//! on load, so they're never written.

use std::collections::HashMap;
use std::io::{Read, Write};

use crate::chest::Chest;
use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ, VOLUME};
use crate::entity::DroppedItem;
use crate::furnace::{Facing, Furnace};
use crate::item::{ItemStack, ItemType};
use crate::mob::SavedMob;
use crate::torch::TorchPlacement;

const BIOME_BYTES: usize = CHUNK_SX * CHUNK_SZ;
/// Current chunk-record version. Extra sections (item entities, furnaces) are
/// gated by flag bits and appended at the end, so adding one keeps the version
/// stable: old code ignores trailing bytes it doesn't recognise, and new code
/// reads a missing section as empty when its flag is clear.
const CHUNK_REC_VERSION: u8 = 2;
const FLAG_HAS_WATER: u8 = 0x01;
const FLAG_HAS_ENTITIES: u8 = 0x02;
const FLAG_HAS_FURNACES: u8 = 0x04;
const FLAG_HAS_CHESTS: u8 = 0x08;
const FLAG_HAS_TORCHES: u8 = 0x10;
const FLAG_HAS_MOBS: u8 = 0x20;
const FLAG_HAS_MODEL_CELLS: u8 = 0x40;
const FLAG_HAS_MODEL_FACINGS: u8 = 0x80;

/// Owned, send-able copy of the per-chunk save data. The game thread builds one
/// of these (a cheap array clone) and hands it to the I/O thread, which does the
/// expensive compression off the game loop.
pub struct ChunkSnapshot {
    pub pos: ChunkPos,
    pub blocks: Box<[u8]>,
    pub biomes: Box<[u8]>,
    pub water: Option<Box<[u8]>>,
    /// Item entities resting in this chunk, captured at save time so their
    /// lifetime timers persist with the chunk. Empty for the common case.
    pub entities: Vec<DroppedItem>,
    /// Furnace block-entities in this chunk, keyed by local block index, so their
    /// contents + smelting progress persist. Empty for the common chunk.
    pub furnaces: HashMap<u16, Furnace>,
    /// Chest block-entities in this chunk, keyed by local block index, so their
    /// contents persist. Empty for the common chunk.
    pub chests: HashMap<u16, Chest>,
    /// Torch orientations in this chunk, keyed by local block index, so a wall vs
    /// floor torch reloads the way it was placed. Empty for the common chunk.
    pub torches: HashMap<u16, TorchPlacement>,
    /// Multi-cell bbmodel occupancy: each non-zero authored footprint offset, so a
    /// placed multi-block (the workbench) reloads as one object. Empty for the common
    /// chunk. See `Chunk::model_cells`.
    pub model_cells: HashMap<u16, [u8; 3]>,
    /// Per-cell facing for oriented bbmodel blocks, keyed like `model_cells`. Empty for
    /// old/non-directional model placements. See `Chunk::model_facings`.
    pub model_facings: HashMap<u16, Facing>,
    /// Mobs resting in this chunk, captured at save time so a passive owl reloads
    /// where it was left. Like [`entities`](Self::entities) these don't live in the
    /// `Chunk`, so the world save paths set this from the live mob set. Empty for the
    /// common chunk.
    pub mobs: Vec<SavedMob>,
}

impl ChunkSnapshot {
    /// Snapshot a chunk's terrain with no entities or mobs attached. The world save
    /// paths set [`entities`](Self::entities) / [`mobs`](Self::mobs) afterwards from the
    /// active item and mob sets.
    pub fn from_chunk(c: &Chunk) -> Self {
        Self {
            pos: ChunkPos::new(c.cx, c.cz),
            blocks: Box::from(c.blocks_slice()),
            biomes: Box::from(c.biomes_slice()),
            water: c.water_slice().map(Box::from),
            entities: Vec::new(),
            furnaces: c.furnaces().clone(),
            chests: c.chests().clone(),
            torches: c.torches().clone(),
            model_cells: c.model_cells().clone(),
            model_facings: c.model_facings().clone(),
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

/// Little-endian append helpers over `Vec<u8>`.
pub trait Writer {
    fn put_u8(&mut self, v: u8);
    fn put_u16(&mut self, v: u16);
    fn put_u32(&mut self, v: u32);
    fn put_u64(&mut self, v: u64);
    fn put_f32(&mut self, v: f32);
}

impl Writer for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_u64(&mut self, v: u64) {
        self.extend_from_slice(&v.to_le_bytes());
    }
    fn put_f32(&mut self, v: f32) {
        self.extend_from_slice(&v.to_le_bytes());
    }
}

/// Encode one inventory/container slot as `[item id, count]`, with `[0, 0]` for an
/// empty or absent slot. Shared by the `level` (inventory/cursor) and `furnace`
/// codecs so the 2-byte slot format lives in exactly one place.
pub fn put_item_slot(buf: &mut Vec<u8>, slot: Option<ItemStack>) {
    match slot {
        Some(s) if !s.is_empty() => {
            buf.put_u8(s.item.id());
            buf.put_u8(s.count);
        }
        _ => {
            buf.put_u8(0);
            buf.put_u8(0);
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
    buf.put_u16(n as u16);
    let mut entries: Vec<(&u16, &T)> = map.iter().take(n).collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (idx, rec) in entries {
        buf.put_u16(*idx);
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

/// Compress a chunk snapshot into a record: `[version, flags, blocks, biomes,
/// water?, entities?]`, zlib-deflated. The entity list is appended only when the
/// chunk holds drops (`FLAG_HAS_ENTITIES`), so terrain-only chunks pay nothing.
pub fn encode_snapshot(s: &ChunkSnapshot) -> Vec<u8> {
    let extra = s.water.as_ref().map_or(0, |w| w.len());
    let mut payload = Vec::with_capacity(2 + s.blocks.len() + s.biomes.len() + extra);
    payload.put_u8(CHUNK_REC_VERSION);
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
    if !s.chests.is_empty() {
        flags |= FLAG_HAS_CHESTS;
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
    payload.put_u8(flags);
    payload.extend_from_slice(&s.blocks);
    payload.extend_from_slice(&s.biomes);
    if let Some(w) = &s.water {
        payload.extend_from_slice(w);
    }
    if !s.entities.is_empty() {
        super::entities::put_entities(&mut payload, &s.entities);
    }
    if !s.furnaces.is_empty() {
        super::furnace::put_furnaces(&mut payload, &s.furnaces);
    }
    if !s.chests.is_empty() {
        super::chest::put_chests(&mut payload, &s.chests);
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
            buf.put_u8(off[0]);
            buf.put_u8(off[1]);
            buf.put_u8(off[2]);
        });
    }
    if !s.model_facings.is_empty() {
        put_indexed(&mut payload, &s.model_facings, 1, |buf, facing| {
            buf.put_u8(facing.to_u8());
        });
    }
    deflate(&payload)
}

/// Decode a compressed chunk record into a `Chunk` at `(cx, cz)` plus any item
/// entities and mobs stored with it. `None` on corrupt / wrong-version /
/// wrong-length data.
pub fn decode_chunk(
    cx: i32,
    cz: i32,
    blob: &[u8],
) -> Option<(Chunk, Vec<DroppedItem>, Vec<SavedMob>)> {
    let payload = inflate(blob)?;
    let mut r = Reader::new(&payload);
    if r.u8()? != CHUNK_REC_VERSION {
        return None;
    }
    let flags = r.u8()?;
    let blocks = r.bytes(VOLUME)?.to_vec().into_boxed_slice();
    let biomes = r.bytes(BIOME_BYTES)?;
    let water = if flags & FLAG_HAS_WATER != 0 {
        Some(r.bytes(VOLUME)?.to_vec().into_boxed_slice())
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
    let chests = if flags & FLAG_HAS_CHESTS != 0 {
        super::chest::get_chests(&mut r)?
    } else {
        HashMap::new()
    };
    let torches = if flags & FLAG_HAS_TORCHES != 0 {
        super::torch::get_torches(&mut r)?
    } else {
        HashMap::new()
    };
    let mobs = if flags & FLAG_HAS_MOBS != 0 {
        super::mobs::get_mobs(&mut r)?
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
    Some((
        Chunk::from_saved(
            cx,
            cz,
            blocks,
            biomes,
            water,
            furnaces,
            chests,
            torches,
            model_cells,
            model_facings,
        ),
        entities,
        mobs,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::mathh::Vec3;

    #[test]
    fn chunk_record_roundtrips() {
        let mut c = Chunk::new(-3, 7);
        c.set_block(1, 64, 2, Block::Stone);
        c.set_block(0, 70, 0, Block::Grass);
        c.set_water(5, 65, 5, Block::Water, 0x23);
        c.set_biome(4, 4, 9);

        let snap = ChunkSnapshot::from_chunk(&c);
        let blob = encode_snapshot(&snap);
        let (back, entities, mobs) = decode_chunk(-3, 7, &blob).expect("decodes");

        assert_eq!(back.cx, -3);
        assert_eq!(back.cz, 7);
        assert_eq!(back.block_raw(1, 64, 2), Block::Stone.id());
        assert_eq!(back.block_raw(0, 70, 0), Block::Grass.id());
        assert_eq!(back.block_raw(5, 65, 5), Block::Water.id());
        assert_eq!(back.water_meta(5, 65, 5), 0x23);
        assert_eq!(back.biome_at(4, 4), 9);
        // Heightmap is recomputed, not stored.
        assert_eq!(back.surface_y(0, 0), 70);
        assert!(!back.modified);
        assert!(entities.is_empty(), "no entities attached");
        assert!(mobs.is_empty(), "no mobs attached");
    }

    #[test]
    fn chunk_record_roundtrips_entities() {
        let mut c = Chunk::new(2, 2);
        c.set_block(8, 64, 8, Block::Dirt);
        let mut snap = ChunkSnapshot::from_chunk(&c);
        let mut drop = DroppedItem::new(
            Vec3::new(40.5, 65.0, 40.5),
            ItemStack::new(ItemType::Stone, 7),
            1,
        );
        drop.ticks_lived = 1234;
        snap.entities.push(drop);

        let blob = encode_snapshot(&snap);
        let (_back, entities, _mobs) = decode_chunk(2, 2, &blob).expect("decodes");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].stack, ItemStack::new(ItemType::Stone, 7));
        assert_eq!(
            entities[0].ticks_lived, 1234,
            "remaining lifetime survives the save"
        );
    }

    #[test]
    fn chunk_record_roundtrips_mobs() {
        let mut c = Chunk::new(-1, 4);
        c.set_block(8, 64, 8, Block::Dirt);
        let mut snap = ChunkSnapshot::from_chunk(&c);
        snap.mobs.push(SavedMob {
            kind: crate::mob::Mob::Owl,
            pos: Vec3::new(-12.5, 65.0, 72.25),
            yaw: 1.75,
        });

        let blob = encode_snapshot(&snap);
        let (_back, _entities, mobs) = decode_chunk(-1, 4, &blob).expect("decodes");
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
    fn chunk_record_roundtrips_furnaces() {
        let mut c = Chunk::new(1, 1);
        c.set_block(2, 65, 3, Block::Furnace);
        c.insert_furnace(
            2,
            65,
            3,
            crate::furnace::Furnace {
                input: Some(ItemStack::new(ItemType::RawCopper, 12)),
                fuel: Some(ItemStack::new(ItemType::Coal, 1)),
                output: None,
                cook_progress: 200,
                burn_remaining: 1000,
                burn_max: 4800,
                facing: crate::furnace::Facing::West,
            },
        );

        let blob = encode_snapshot(&ChunkSnapshot::from_chunk(&c));
        let (back, _entities, _mobs) = decode_chunk(1, 1, &blob).expect("decodes");

        assert_eq!(back.block_raw(2, 65, 3), Block::Furnace.id());
        let f = back.furnace_at(2, 65, 3).expect("furnace restored");
        assert_eq!(f.input, Some(ItemStack::new(ItemType::RawCopper, 12)));
        assert_eq!(f.fuel, Some(ItemStack::new(ItemType::Coal, 1)));
        assert_eq!(f.cook_progress, 200);
        assert_eq!(f.burn_remaining, 1000);
        assert_eq!(f.facing, crate::furnace::Facing::West, "facing persists");
        assert!(f.is_lit(), "a saved burning furnace reloads lit");
    }

    #[test]
    fn chunk_record_roundtrips_chests() {
        let mut c = Chunk::new(4, -2);
        c.set_block(9, 66, 1, Block::Chest);
        let mut chest = crate::chest::Chest {
            facing: crate::furnace::Facing::South,
            ..crate::chest::Chest::default()
        };
        chest.slots[0] = Some(ItemStack::new(ItemType::Stone, 64));
        chest.slots[26] = Some(ItemStack::new(ItemType::OakLog, 5));
        c.insert_chest(9, 66, 1, chest);

        let blob = encode_snapshot(&ChunkSnapshot::from_chunk(&c));
        let (back, _entities, _mobs) = decode_chunk(4, -2, &blob).expect("decodes");

        assert_eq!(back.block_raw(9, 66, 1), Block::Chest.id());
        let got = back.chest_at(9, 66, 1).expect("chest restored");
        assert_eq!(got.slots[0], Some(ItemStack::new(ItemType::Stone, 64)));
        assert_eq!(got.slots[26], Some(ItemStack::new(ItemType::OakLog, 5)));
        assert_eq!(got.slots[5], None);
        assert_eq!(got.facing, crate::furnace::Facing::South, "facing persists");
    }

    #[test]
    fn chunk_record_roundtrips_torches() {
        use crate::torch::TorchPlacement;
        let mut c = Chunk::new(6, 6);
        c.set_block(3, 67, 4, Block::Torch);
        c.insert_torch(3, 67, 4, TorchPlacement::East);
        c.set_block(3, 68, 4, Block::Torch);
        c.insert_torch(3, 68, 4, TorchPlacement::Floor);

        let blob = encode_snapshot(&ChunkSnapshot::from_chunk(&c));
        let (back, _entities, _mobs) = decode_chunk(6, 6, &blob).expect("decodes");

        assert_eq!(back.block_raw(3, 67, 4), Block::Torch.id());
        assert_eq!(
            back.torch_placement(3, 67, 4),
            TorchPlacement::East,
            "wall mount persists"
        );
        assert_eq!(
            back.torch_placement(3, 68, 4),
            TorchPlacement::Floor,
            "floor mount persists"
        );
        // A cell with no torch reads the Floor default.
        assert_eq!(back.torch_placement(0, 0, 0), TorchPlacement::Floor);
    }

    #[test]
    fn chunk_record_roundtrips_model_cells() {
        // A placed multi-block records authored footprint offsets and per-cell facing;
        // both must survive a save/load so the block reloads as one object.
        let mut c = Chunk::new(2, 3);
        c.set_block(5, 64, 5, Block::FurnitureWorkbench);
        c.set_block(6, 64, 5, Block::FurnitureWorkbench);
        c.set_model_offset(6, 64, 5, [1, 0, 0]);
        c.set_model_facing(6, 64, 5, Facing::East);
        c.set_block(5, 65, 5, Block::FurnitureWorkbench);
        c.set_model_offset(5, 65, 5, [0, 1, 0]);
        c.set_model_facing(5, 65, 5, Facing::East);
        c.set_model_facing(5, 64, 5, Facing::East);

        let blob = encode_snapshot(&ChunkSnapshot::from_chunk(&c));
        let (back, _entities, _mobs) = decode_chunk(2, 3, &blob).expect("decodes");

        assert_eq!(back.block_raw(6, 64, 5), Block::FurnitureWorkbench.id());
        assert_eq!(back.model_offset(6, 64, 5), [1, 0, 0], "x-offset persists");
        assert_eq!(back.model_offset(5, 65, 5), [0, 1, 0], "y-offset persists");
        assert_eq!(back.model_facing(6, 64, 5), Facing::East, "facing persists");
        assert_eq!(back.model_facing(5, 65, 5), Facing::East, "facing persists");
        assert_eq!(
            back.model_facing(5, 64, 5),
            Facing::East,
            "origin facing persists"
        );
        // The origin cell stores no offset and reads the [0,0,0] default.
        assert_eq!(back.model_offset(5, 64, 5), [0, 0, 0]);
    }

    #[test]
    fn water_free_chunk_omits_water() {
        let mut c = Chunk::new(0, 0);
        c.set_block(8, 64, 8, Block::Dirt);
        let snap = ChunkSnapshot::from_chunk(&c);
        assert!(snap.water.is_none());
        let blob = encode_snapshot(&snap);
        let (back, _, _) = decode_chunk(0, 0, &blob).expect("decodes");
        assert_eq!(back.water_meta(8, 64, 8), 0);
    }

    #[test]
    fn corrupt_blob_is_none() {
        assert!(decode_chunk(0, 0, &[1, 2, 3, 4]).is_none());
        assert!(decode_chunk(0, 0, &[]).is_none());
    }
}
