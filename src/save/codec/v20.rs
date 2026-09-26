//! Section record v20 → v21: item entities and mobs become tagged records.
//!
//! v20 frames every payload, so this step copies the flags, the block cube
//! and every other payload unchanged and rewrites only the entity and mob
//! frames. It walks their v20 layouts as they SHIPPED and writes the v21
//! records under explicit tags, all frozen here — later changes to the live
//! codecs cannot change what it produces. Ids stay disk ids (item slots are
//! copied as stored), so the step needs no palette.

use petramond_util::bytecodec::Reader;

use super::{
    RecordError, FLAG2_HAS_CELL_KV, FLAG2_HAS_CONTAINERS, FLAG3_HAS_BLOCKLIGHT, FLAG3_HAS_SKYLIGHT,
    FLAG_HAS_CELL_STATES, FLAG_HAS_ENTITIES, FLAG_HAS_FLUID, FLAG_HAS_FURNACES, FLAG_HAS_MOBS,
    SECTION,
};
use crate::save::wire::{TaggedWriter, Wire};

const SECTION_VOLUME: usize = 4096;
const WIDE_ID_CAP: usize = 4096;

/// The flag bits a v20 record could carry, per flags byte.
const KNOWN: [u8; 3] = [
    FLAG_HAS_FLUID | FLAG_HAS_ENTITIES | FLAG_HAS_FURNACES | FLAG_HAS_CELL_STATES | FLAG_HAS_MOBS,
    FLAG2_HAS_CELL_KV | FLAG2_HAS_CONTAINERS,
    FLAG3_HAS_SKYLIGHT | FLAG3_HAS_BLOCKLIGHT,
];

/// A payload rewrite: the v20 payload in, the v21 payload out.
type Rewrite = fn(&[u8]) -> Option<Vec<u8>>;

/// Rewrite a v20 record body (after the version byte) as a v21 body.
pub(super) fn upgrade(body: &[u8]) -> Result<Vec<u8>, RecordError> {
    // Offsets are reported against the whole decompressed record, whose
    // version byte precedes `body`.
    let corrupt = |what, at: usize| RecordError::corrupt(SECTION.name, what, at + 1);
    let mut r = Reader::new(body);
    let flags = [0, 1, 2].map(|_| r.u8());
    let [Some(f1), Some(f2), Some(f3)] = flags else {
        return Err(corrupt("flags", r.offset()));
    };
    let unknown = [f1, f2, f3]
        .iter()
        .zip(KNOWN)
        .enumerate()
        .fold(0u32, |acc, (i, (f, known))| {
            acc | (u32::from(f & !known) << (8 * i))
        });
    if unknown != 0 {
        return Err(RecordError::UnknownPayload {
            format: SECTION.name,
            flags: unknown,
        });
    }
    skip_block_cube(&mut r).ok_or_else(|| corrupt("block cube", r.offset()))?;
    let mut out = Vec::with_capacity(body.len() + 64);
    out.extend_from_slice(&body[..r.offset()]);

    let payloads: [(bool, &'static str, Option<Rewrite>); 9] = [
        (f1 & FLAG_HAS_FLUID != 0, "fluid", None),
        (f1 & FLAG_HAS_ENTITIES != 0, "entities", Some(entities)),
        (f1 & FLAG_HAS_FURNACES != 0, "furnaces", None),
        (f1 & FLAG_HAS_CELL_STATES != 0, "cell states", None),
        (f1 & FLAG_HAS_MOBS != 0, "mobs", Some(mobs)),
        (f2 & FLAG2_HAS_CELL_KV != 0, "cell kv", None),
        (f2 & FLAG2_HAS_CONTAINERS != 0, "containers", None),
        (f3 & FLAG3_HAS_SKYLIGHT != 0, "skylight", None),
        (f3 & FLAG3_HAS_BLOCKLIGHT != 0, "block light", None),
    ];
    for (present, what, rewrite) in payloads {
        if !present {
            continue;
        }
        let start = r.offset();
        let len = r.u32().ok_or_else(|| corrupt(what, start))? as usize;
        let payload = r.bytes(len).ok_or_else(|| corrupt(what, start))?;
        let payload = match rewrite {
            Some(rewrite) => rewrite(payload).ok_or_else(|| corrupt(what, start + 4))?,
            None => payload.to_vec(),
        };
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&payload);
    }
    if !r.is_at_end() {
        return Err(corrupt("trailing bytes", r.offset()));
    }
    Ok(out)
}

fn skip_block_cube(r: &mut Reader) -> Option<()> {
    let distinct = r.u16()? as usize;
    if distinct == 0 || distinct > WIDE_ID_CAP {
        return None;
    }
    r.bytes(distinct * 2)?;
    let index_width = if distinct <= 256 { 1 } else { 2 };
    r.bytes(SECTION_VOLUME * index_width).map(|_| ())
}

/// A v20 item slot, copied as stored: `[id u16][count u8][blob len u16][blob]`.
fn slot<'a>(r: &mut Reader<'a>, whole: &'a [u8]) -> Option<&'a [u8]> {
    let start = r.offset();
    r.bytes(3)?;
    let blob = r.u16()? as usize;
    r.bytes(blob)?;
    whole.get(start..r.offset())
}

/// v20: `u16` count; per entity position (3 × f64), velocity (3 × f32),
/// slot, lifetime (u32), spin (f32), motion tag — a lodged item (tag 2) adds
/// its heading (2 × f32) and anchor (3 × u32).
/// v21: `u32` count of tagged records: 1 position, 2 velocity, 3 slot,
/// 4 lifetime, 5 spin, 6 motion tag, 7 lodging (`Option` of heading yaw,
/// pitch and anchor).
fn entities(payload: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(payload);
    let n = r.u16()?;
    let mut out = Vec::with_capacity(payload.len() * 2);
    u32::from(n).put(&mut out);
    for _ in 0..n {
        let pos = r.bytes(24)?;
        let vel = r.bytes(12)?;
        let slot = slot(&mut r, payload)?;
        let ticks = r.bytes(4)?;
        let spin = r.bytes(4)?;
        let motion = r.u8()?;
        let stuck = if motion == 2 { Some(r.bytes(20)?) } else { None };
        let mut w = TaggedWriter::new(&mut out);
        w.raw(1, pos);
        w.raw(2, vel);
        w.raw(3, slot);
        w.raw(4, ticks);
        w.raw(5, spin);
        w.raw(6, &[motion]);
        match stuck {
            Some(lodging) => w.raw(7, &[&[1u8][..], lodging].concat()),
            None => w.raw(7, &[0]),
        }
        w.finish();
    }
    r.is_at_end().then_some(out)
}

/// v20: `u16` count; when non-empty a `u16`-counted key table of
/// `u16`-length strings, then per mob species (u8), position (3 × f64), yaw
/// (f32), a `u16`-counted tag list of `(u16 key index, u8 type, value)` —
/// bool u8, int i64, float f64, string `u32`-length — and a `u8`-counted
/// slot list.
/// v21: the key table as a `u32`-counted list of `u32`-length strings, then
/// a `u32` count of tagged records: 1 species, 2 position, 3 yaw, 4 tags
/// (`u32`-counted `(u16 key index, u8 type, value)`, a bool as 0 or 1),
/// 5 slots (`u32`-counted).
fn mobs(payload: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(payload);
    let n = r.u16()?;
    let mut out = Vec::with_capacity(payload.len() * 2);
    let mut keys = Vec::new();
    if n > 0 {
        for _ in 0..r.u16()? {
            let len = r.u16()? as usize;
            keys.push(r.bytes(len)?);
        }
    }
    (keys.len() as u32).put(&mut out);
    for key in &keys {
        (key.len() as u32).put(&mut out);
        out.extend_from_slice(key);
    }
    u32::from(n).put(&mut out);
    for _ in 0..n {
        let species = r.u8()?;
        let pos = r.bytes(24)?;
        let yaw = r.bytes(4)?;
        let tag_count = r.u16()?;
        let mut tags = Vec::new();
        u32::from(tag_count).put(&mut tags);
        for _ in 0..tag_count {
            tags.extend_from_slice(r.bytes(2)?);
            let kind = r.u8()?;
            tags.push(kind);
            match kind {
                0 => tags.push(u8::from(r.u8()? != 0)),
                1 | 2 => tags.extend_from_slice(r.bytes(8)?),
                3 => {
                    let len = r.u32()?;
                    len.put(&mut tags);
                    tags.extend_from_slice(r.bytes(len as usize)?);
                }
                _ => return None,
            }
        }
        let slot_count = r.u8()?;
        let mut slots = Vec::new();
        u32::from(slot_count).put(&mut slots);
        for _ in 0..slot_count {
            slots.extend_from_slice(slot(&mut r, payload)?);
        }
        let mut w = TaggedWriter::new(&mut out);
        w.raw(1, &[species]);
        w.raw(2, pos);
        w.raw(3, yaw);
        w.raw(4, &tags);
        w.raw(5, &slots);
        w.finish();
    }
    r.is_at_end().then_some(out)
}
