//! (De)serialization for the furnace block-entities stored inside a chunk's save
//! record.
//!
//! Furnaces live with their owning chunk, so this is a helper for the chunk codec
//! rather than a standalone file format. A chunk record appends a u16-length-
//! prefixed list of these — each keyed by its local block index — after the
//! block/biome/water/entity data when `FLAG_HAS_FURNACES` is set; see
//! `save::codec`. Mirrors `save::entities`.

use std::collections::HashMap;

use crate::furnace::{Facing, Furnace};
use crate::save::codec::{get_item_slot, put_item_slot, Reader, Writer};

/// Bytes per serialized furnace: idx(2) + 3 slots × (id 1 + count 1) + cook(2) +
/// burn(2) + burn_max(2) + facing(1).
const FURNACE_BYTES: usize = 2 + 6 + 6 + 1;

/// Append a `u16`-length-prefixed list of `(local index, furnace)` records to
/// `buf`, in ascending index order so identical state encodes identically.
pub fn put_furnaces(buf: &mut Vec<u8>, furnaces: &HashMap<u16, Furnace>) {
    let n = furnaces.len().min(u16::MAX as usize);
    buf.reserve(2 + n * FURNACE_BYTES);
    buf.put_u16(n as u16);
    let mut entries: Vec<(&u16, &Furnace)> = furnaces.iter().take(n).collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (idx, f) in entries {
        buf.put_u16(*idx);
        put_item_slot(buf, f.input);
        put_item_slot(buf, f.fuel);
        put_item_slot(buf, f.output);
        buf.put_u16(f.cook_progress);
        buf.put_u16(f.burn_remaining);
        buf.put_u16(f.burn_max);
        buf.put_u8(f.facing.to_u8());
    }
}

/// Read the furnace list written by [`put_furnaces`]. `None` on truncated input.
pub fn get_furnaces(r: &mut Reader) -> Option<HashMap<u16, Furnace>> {
    let n = r.u16()? as usize;
    let mut out = HashMap::with_capacity(n.min(256));
    for _ in 0..n {
        let idx = r.u16()?;
        let input = get_item_slot(r)?;
        let fuel = get_item_slot(r)?;
        let output = get_item_slot(r)?;
        let cook_progress = r.u16()?;
        let burn_remaining = r.u16()?;
        let burn_max = r.u16()?;
        let facing = Facing::from_u8(r.u8()?);
        out.insert(
            idx,
            Furnace {
                input,
                fuel,
                output,
                cook_progress,
                burn_remaining,
                burn_max,
                facing,
            },
        );
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{ItemStack, ItemType};

    #[test]
    fn furnaces_roundtrip_through_a_buffer() {
        let mut map = HashMap::new();
        map.insert(
            5u16,
            Furnace {
                input: Some(ItemStack::new(ItemType::RawIron, 30)),
                fuel: Some(ItemStack::new(ItemType::Coal, 2)),
                output: Some(ItemStack::new(ItemType::IronIngot, 7)),
                cook_progress: 123,
                burn_remaining: 456,
                burn_max: 4800,
                facing: Facing::East,
            },
        );
        map.insert(60000u16, Furnace::default());

        let mut buf = Vec::new();
        put_furnaces(&mut buf, &map);
        let mut r = Reader::new(&buf);
        let got = get_furnaces(&mut r).expect("decodes");
        assert_eq!(got, map, "furnace state survives the round-trip");
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_furnaces(&mut buf, &HashMap::new());
        let mut r = Reader::new(&buf);
        assert!(get_furnaces(&mut r).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        let mut buf = Vec::new();
        buf.put_u16(1); // claims one furnace, provides no body
        let mut r = Reader::new(&buf);
        assert!(get_furnaces(&mut r).is_none());
    }
}
