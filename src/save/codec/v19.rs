use petramond_persist::bytecodec::Reader;

use super::{
    RecordError, FLAG2_HAS_CELL_KV, FLAG2_HAS_CONTAINERS, FLAG3_HAS_BLOCKLIGHT, FLAG3_HAS_SKYLIGHT,
    FLAG_HAS_CELL_STATES, FLAG_HAS_ENTITIES, FLAG_HAS_FLUID, FLAG_HAS_FURNACES, FLAG_HAS_MOBS,
    SECTION,
};

const SECTION_VOLUME: usize = 4096;
const WIDE_ID_CAP: usize = 4096;
const SHAPE_STATE_MAX: usize = 8;

const KNOWN: [u8; 3] = [
    FLAG_HAS_FLUID | FLAG_HAS_ENTITIES | FLAG_HAS_FURNACES | FLAG_HAS_CELL_STATES | FLAG_HAS_MOBS,
    FLAG2_HAS_CELL_KV | FLAG2_HAS_CONTAINERS,
    FLAG3_HAS_SKYLIGHT | FLAG3_HAS_BLOCKLIGHT,
];

type Skip = fn(&mut Reader) -> Option<()>;

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
    let mut out = Vec::with_capacity(body.len() + 36);
    out.extend_from_slice(&[f1, f2, f3]);
    let cube_start = r.offset();
    skip_block_cube(&mut r).ok_or_else(|| corrupt("block cube", r.offset()))?;
    out.extend_from_slice(&body[cube_start..r.offset()]);

    let payloads: [(bool, &'static str, Skip); 9] = [
        (f1 & FLAG_HAS_FLUID != 0, "fluid", |r| {
            skip(r, SECTION_VOLUME)
        }),
        (f1 & FLAG_HAS_ENTITIES != 0, "entities", skip_entities),
        (f1 & FLAG_HAS_FURNACES != 0, "furnaces", skip_furnaces),
        (
            f1 & FLAG_HAS_CELL_STATES != 0,
            "cell states",
            skip_cell_states,
        ),
        (f1 & FLAG_HAS_MOBS != 0, "mobs", skip_mobs),
        (f2 & FLAG2_HAS_CELL_KV != 0, "cell kv", skip_cell_kv),
        (
            f2 & FLAG2_HAS_CONTAINERS != 0,
            "containers",
            skip_containers,
        ),
        (f3 & FLAG3_HAS_SKYLIGHT != 0, "skylight", |r| {
            skip(r, SECTION_VOLUME)
        }),
        (f3 & FLAG3_HAS_BLOCKLIGHT != 0, "block light", |r| {
            skip(r, SECTION_VOLUME * 2)
        }),
    ];
    for (present, what, walk) in payloads {
        if !present {
            continue;
        }
        let start = r.offset();
        walk(&mut r).ok_or_else(|| corrupt(what, r.offset()))?;
        let bytes = &body[start..r.offset()];
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    if !r.is_at_end() {
        return Err(corrupt("trailing bytes", r.offset()));
    }
    Ok(out)
}

fn skip(r: &mut Reader, n: usize) -> Option<()> {
    r.bytes(n).map(|_| ())
}

fn skip_block_cube(r: &mut Reader) -> Option<()> {
    let distinct = r.u16()? as usize;
    if distinct == 0 || distinct > WIDE_ID_CAP {
        return None;
    }
    skip(r, distinct * 2)?;
    let index_width = if distinct <= 256 { 1 } else { 2 };
    skip(r, SECTION_VOLUME * index_width)
}

fn skip_item_slot(r: &mut Reader) -> Option<()> {
    skip(r, 3)?;
    let blob = r.u16()? as usize;
    skip(r, blob)
}

fn skip_indexed(r: &mut Reader, mut body: impl FnMut(&mut Reader) -> Option<()>) -> Option<()> {
    for _ in 0..r.u16()? {
        skip(r, 2)?;
        body(r)?;
    }
    Some(())
}

/// Per entity we skip pos (3xf64), vel (3xf32), the slot, lifetime (u32), spin (f32) and the
/// motion tag. A lodged item, tag 2, carries heading (2xf32) and anchor (3xu32) too.
fn skip_entities(r: &mut Reader) -> Option<()> {
    for _ in 0..r.u16()? {
        skip(r, 24 + 12)?;
        skip_item_slot(r)?;
        skip(r, 4 + 4)?;
        if r.u8()? == 2 {
            skip(r, 8 + 12)?;
        }
    }
    Some(())
}

fn skip_furnaces(r: &mut Reader) -> Option<()> {
    skip_indexed(r, |r| skip(r, 6))
}

fn skip_cell_states(r: &mut Reader) -> Option<()> {
    skip_indexed(r, |r| {
        let len = r.u8()? as usize;
        skip(r, 1)?;
        if len > SHAPE_STATE_MAX {
            return None;
        }
        skip(r, len)
    })
}

fn skip_mobs(r: &mut Reader) -> Option<()> {
    let n = r.u16()?;
    if n == 0 {
        return Some(());
    }
    for _ in 0..r.u16()? {
        let len = r.u16()? as usize;
        skip(r, len)?;
    }
    for _ in 0..n {
        skip(r, 1 + 24 + 4)?;
        for _ in 0..r.u16()? {
            skip(r, 2)?;
            match r.u8()? {
                0 => skip(r, 1)?,
                1 | 2 => skip(r, 8)?,
                3 => {
                    let len = r.u32()? as usize;
                    skip(r, len)?;
                }
                _ => return None,
            }
        }
        for _ in 0..r.u8()? {
            skip_item_slot(r)?;
        }
    }
    Some(())
}

fn skip_cell_kv(r: &mut Reader) -> Option<()> {
    skip_indexed(r, |r| {
        for _ in 0..r.u16()? {
            let key = r.u16()? as usize;
            skip(r, key)?;
            let value = r.u32()? as usize;
            skip(r, value)?;
        }
        Some(())
    })
}

fn skip_containers(r: &mut Reader) -> Option<()> {
    skip_indexed(r, |r| {
        for _ in 0..r.u8()? {
            skip_item_slot(r)?;
        }
        Some(())
    })
}
