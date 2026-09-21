use super::bytes::{self, Reader};
use crate::schematic::{CellData, SavedStack};
use rustc_hash::FxHashMap;
use std::collections::{BTreeMap, BTreeSet};

const MAX_NAME_BYTES: usize = 256;
const MAX_STATE_IDS: usize = 4;
const MAX_KV_ENTRIES: usize = 256;
const MAX_KV_VALUE_BYTES: usize = 65_536;
const MAX_STACK_DATA_BYTES: usize = 1024;
/// Most distinct names one palette entry can reference: its block, its
/// state ids, its instance keys and one item per container slot.
const MAX_NAMES_PER_ENTRY: usize =
    1 + MAX_STATE_IDS + MAX_KV_ENTRIES + petramond_world::container::MAX_CONTAINER_SLOTS;

pub(super) fn encode(palette: &[&CellData], out: &mut Vec<u8>) {
    let mut names = BTreeSet::new();
    for d in palette {
        names.insert(d.block.as_str());
        names.extend(d.state_ids.values().map(String::as_str));
        names.extend(d.kv.keys().map(String::as_str));
        if let Some(slots) = &d.container {
            names.extend(slots.iter().flatten().map(|s| s.item.as_str()));
        }
    }
    let indices: FxHashMap<_, _> = names.iter().enumerate().map(|(i, s)| (*s, i)).collect();
    bytes::uint(out, names.len());
    for name in names {
        bytes::blob(out, name.as_bytes());
    }
    bytes::uint(out, palette.len());
    for d in palette {
        bytes::uint(out, indices[d.block.as_str()]);
        let flags = u8::from(!d.state.is_empty())
            | (u8::from(!d.state_ids.is_empty()) << 1)
            | (u8::from(d.fluid != 0) << 2)
            | (u8::from(!d.kv.is_empty()) << 3)
            | (u8::from(d.container.is_some()) << 4)
            | (u8::from(d.furnace.is_some()) << 5);
        out.push(flags);
        if flags & 1 != 0 {
            bytes::blob(out, &d.state);
        }
        if flags & 2 != 0 {
            bytes::uint(out, d.state_ids.len());
            for (&offset, name) in &d.state_ids {
                out.push(offset);
                bytes::uint(out, indices[name.as_str()]);
            }
        }
        if flags & 4 != 0 {
            out.push(d.fluid);
        }
        if flags & 8 != 0 {
            bytes::uint(out, d.kv.len());
            for (key, value) in &d.kv {
                bytes::uint(out, indices[key.as_str()]);
                bytes::blob(out, value);
            }
        }
        if let Some(slots) = &d.container {
            bytes::uint(out, slots.len());
            bytes::uint(out, slots.iter().flatten().count());
            let mut next = 0;
            for (i, stack) in slots.iter().enumerate() {
                if let Some(s) = stack {
                    bytes::uint(out, i - next);
                    next = i + 1;
                    bytes::uint(out, indices[s.item.as_str()]);
                    out.push(s.count);
                    bytes::blob(out, &s.data);
                }
            }
        }
        if let Some(furnace) = d.furnace {
            for n in furnace {
                bytes::uint(out, usize::from(n));
            }
        }
    }
}

pub(super) fn decode<R: std::io::Read>(
    r: &mut Reader<R>,
    cell_count: usize,
) -> Result<Vec<CellData>, String> {
    let count = r.uint(cell_count * MAX_NAMES_PER_ENTRY)?;
    let mut names = Vec::new();
    for _ in 0..count {
        let name = String::from_utf8(r.blob(MAX_NAME_BYTES)?)
            .map_err(|_| "Invalid schematic name encoding")?;
        if names
            .last()
            .is_some_and(|previous: &String| previous.as_str() >= name.as_str())
        {
            return Err("Unsorted schematic name table".into());
        }
        names.push(name);
    }
    let name = |r: &mut Reader<R>| -> Result<String, String> {
        let index = r.uint(names.len().saturating_sub(1))?;
        names
            .get(index)
            .cloned()
            .ok_or_else(|| "Invalid schematic name reference".into())
    };
    let count = r.uint(cell_count)?;
    if count == 0 {
        return Err("Empty schematic palette".into());
    }
    let mut palette = Vec::with_capacity(count);
    for _ in 0..count {
        let block = name(r)?;
        let flags = r.byte()?;
        if flags & !63 != 0 {
            return Err("Unknown schematic cell flags".into());
        }
        let state = if flags & 1 != 0 {
            r.blob(petramond_world::block::SHAPE_STATE_MAX)?.to_vec()
        } else {
            Vec::new()
        };
        let mut state_ids = BTreeMap::new();
        if flags & 2 != 0 {
            for _ in 0..r.uint(MAX_STATE_IDS)? {
                let offset = r.byte()?;
                if state_ids.insert(offset, name(r)?).is_some() {
                    return Err("Duplicate shape reference".into());
                }
            }
        }
        let fluid = if flags & 4 != 0 { r.byte()? } else { 0 };
        let mut kv = BTreeMap::new();
        if flags & 8 != 0 {
            for _ in 0..r.uint(MAX_KV_ENTRIES)? {
                let key = name(r)?;
                let value = r.blob(MAX_KV_VALUE_BYTES)?.to_vec();
                if kv.insert(key, value).is_some() {
                    return Err("Duplicate instance key".into());
                }
            }
        }
        let container = if flags & 16 != 0 {
            let n = r.uint(petramond_world::container::MAX_CONTAINER_SLOTS)?;
            let mut slots = vec![None; n];
            let mut next = 0;
            for _ in 0..r.uint(n)? {
                let index = next + r.uint(n)?;
                let slot = slots.get_mut(index).ok_or("Invalid container slot")?;
                next = index + 1;
                *slot = Some(SavedStack {
                    item: name(r)?,
                    count: r.byte()?,
                    data: r.blob(MAX_STACK_DATA_BYTES)?.to_vec(),
                });
            }
            Some(slots)
        } else {
            None
        };
        let furnace = if flags & 32 != 0 {
            Some([
                r.uint(65535)? as u16,
                r.uint(65535)? as u16,
                r.uint(65535)? as u16,
            ])
        } else {
            None
        };
        let d = CellData {
            block,
            state,
            state_ids,
            fluid,
            kv,
            container,
            furnace,
        };
        d.validate()?;
        palette.push(d);
    }
    Ok(palette)
}
