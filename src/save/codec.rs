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
use crate::furnace::Furnace;
use crate::item::{ItemStack, ItemType};

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
}

impl ChunkSnapshot {
    /// Snapshot a chunk's terrain with no entities attached. The world save paths
    /// set [`entities`](Self::entities) afterwards from the active item list.
    pub fn from_chunk(c: &Chunk) -> Self {
        Self {
            pos: ChunkPos::new(c.cx, c.cz),
            blocks: Box::from(c.blocks_slice()),
            biomes: Box::from(c.biomes_slice()),
            water: c.water_slice().map(Box::from),
            entities: Vec::new(),
            furnaces: c.furnaces().clone(),
            chests: c.chests().clone(),
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
    deflate(&payload)
}

/// Decode a compressed chunk record into a `Chunk` at `(cx, cz)` plus any item
/// entities stored with it. `None` on corrupt / wrong-version / wrong-length
/// data.
pub fn decode_chunk(cx: i32, cz: i32, blob: &[u8]) -> Option<(Chunk, Vec<DroppedItem>)> {
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
    Some((
        Chunk::from_saved(cx, cz, blocks, biomes, water, furnaces, chests),
        entities,
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
        let (back, entities) = decode_chunk(-3, 7, &blob).expect("decodes");

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
        let (_back, entities) = decode_chunk(2, 2, &blob).expect("decodes");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].stack, ItemStack::new(ItemType::Stone, 7));
        assert_eq!(entities[0].ticks_lived, 1234, "remaining lifetime survives the save");
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
        let (back, _entities) = decode_chunk(1, 1, &blob).expect("decodes");

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
        let (back, _entities) = decode_chunk(4, -2, &blob).expect("decodes");

        assert_eq!(back.block_raw(9, 66, 1), Block::Chest.id());
        let got = back.chest_at(9, 66, 1).expect("chest restored");
        assert_eq!(got.slots[0], Some(ItemStack::new(ItemType::Stone, 64)));
        assert_eq!(got.slots[26], Some(ItemStack::new(ItemType::OakLog, 5)));
        assert_eq!(got.slots[5], None);
        assert_eq!(got.facing, crate::furnace::Facing::South, "facing persists");
    }

    #[test]
    fn water_free_chunk_omits_water() {
        let mut c = Chunk::new(0, 0);
        c.set_block(8, 64, 8, Block::Dirt);
        let snap = ChunkSnapshot::from_chunk(&c);
        assert!(snap.water.is_none());
        let blob = encode_snapshot(&snap);
        let (back, _) = decode_chunk(0, 0, &blob).expect("decodes");
        assert_eq!(back.water_meta(8, 64, 8), 0);
    }

    #[test]
    fn corrupt_blob_is_none() {
        assert!(decode_chunk(0, 0, &[1, 2, 3, 4]).is_none());
        assert!(decode_chunk(0, 0, &[]).is_none());
    }
}
