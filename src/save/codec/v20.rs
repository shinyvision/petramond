use petramond_persist::bytecodec::Reader;

use super::{
    RecordError, FLAG2_HAS_CELL_KV, FLAG2_HAS_CONTAINERS, FLAG3_HAS_BLOCKLIGHT, FLAG3_HAS_SKYLIGHT,
    FLAG_HAS_CELL_STATES, FLAG_HAS_ENTITIES, FLAG_HAS_FLUID, FLAG_HAS_FURNACES, FLAG_HAS_MOBS,
    SECTION,
};
use crate::save::wire::{TaggedWriter, Wire};

const SECTION_VOLUME: usize = 4096;
const WIDE_ID_CAP: usize = 4096;

const KNOWN: [u8; 3] = [
    FLAG_HAS_FLUID | FLAG_HAS_ENTITIES | FLAG_HAS_FURNACES | FLAG_HAS_CELL_STATES | FLAG_HAS_MOBS,
    FLAG2_HAS_CELL_KV | FLAG2_HAS_CONTAINERS,
    FLAG3_HAS_SKYLIGHT | FLAG3_HAS_BLOCKLIGHT,
];

type Rewrite = fn(&[u8]) -> Option<Vec<u8>>;

pub(super) fn upgrade(body: &[u8]) -> Result<Vec<u8>, RecordError> {
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

fn slot<'a>(r: &mut Reader<'a>, whole: &'a [u8]) -> Option<&'a [u8]> {
    let start = r.offset();
    r.bytes(3)?;
    let blob = r.u16()? as usize;
    r.bytes(blob)?;
    whole.get(start..r.offset())
}

/// v20 entity, in order: pos 3xf64, vel 3xf32, slot, lifetime u32, spin f32, motion tag. Count u16
/// back then.
/// Lodged is motion==2, adds heading 2xf32 + anchor 3xu32.
/// v21 bumped count to u32 and turned fields into tags: 1 pos 2 vel 3 slot 4 lifetime 5 spin 6
/// motion 7 lodging.
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
        let stuck = if motion == 2 {
            Some(r.bytes(20)?)
        } else {
            None
        };
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

/// v20 layout. u16 count up front, then if nonzero a key table: u16 count, u16-len strings.
/// Per mob: species u8, pos 3xf64, yaw f32, tags (u16 count, key idx u16, type u8, value - bool u8,
/// int i64, float f64, string u32-len), slots (u8 count).
/// v21 made all the counts and string lengths u32. Records tagged: 1 species, 2 pos, 3 yaw,
/// 4 tags (bool 0/1), 5 slots.
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
