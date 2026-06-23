//! (De)serialization for the chest block-entities stored inside a chunk's save
//! record.
//!
//! Chests live with their owning chunk, so this is a helper for the chunk codec
//! rather than a standalone file format. A chunk record appends a u16-length-
//! prefixed list of these — each keyed by its local block index — after the
//! block/biome/water/entity/furnace data when `FLAG_HAS_CHESTS` is set; see
//! `save::codec`. Mirrors `save::furnace`.

use std::collections::HashMap;

use crate::chest::{Chest, CHEST_SLOTS};
use crate::furnace::Facing;
use crate::save::codec::{get_item_slot, put_item_slot, Reader, Writer};

/// Bytes per serialized chest: idx(2) + 27 slots × (id 1 + count 1) + facing(1).
const CHEST_BYTES: usize = 2 + CHEST_SLOTS * 2 + 1;

/// Append a `u16`-length-prefixed list of `(local index, chest)` records to `buf`,
/// in ascending index order so identical state encodes identically.
pub fn put_chests(buf: &mut Vec<u8>, chests: &HashMap<u16, Chest>) {
    let n = chests.len().min(u16::MAX as usize);
    buf.reserve(2 + n * CHEST_BYTES);
    buf.put_u16(n as u16);
    let mut entries: Vec<(&u16, &Chest)> = chests.iter().take(n).collect();
    entries.sort_by_key(|(idx, _)| **idx);
    for (idx, c) in entries {
        buf.put_u16(*idx);
        for slot in c.slots {
            put_item_slot(buf, slot);
        }
        buf.put_u8(c.facing.to_u8());
    }
}

/// Read the chest list written by [`put_chests`]. `None` on truncated input.
pub fn get_chests(r: &mut Reader) -> Option<HashMap<u16, Chest>> {
    let n = r.u16()? as usize;
    let mut out = HashMap::with_capacity(n.min(256));
    for _ in 0..n {
        let idx = r.u16()?;
        let mut slots = [None; CHEST_SLOTS];
        for slot in slots.iter_mut() {
            *slot = get_item_slot(r)?;
        }
        let facing = Facing::from_u8(r.u8()?);
        out.insert(idx, Chest { slots, facing });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{ItemStack, ItemType};

    #[test]
    fn chests_roundtrip_through_a_buffer() {
        let mut full = Chest {
            facing: Facing::East,
            ..Chest::default()
        };
        full.slots[0] = Some(ItemStack::new(ItemType::Stone, 64));
        full.slots[13] = Some(ItemStack::new(ItemType::OakLog, 3));
        full.slots[26] = Some(ItemStack::new(ItemType::Coal, 17));

        let mut map = HashMap::new();
        map.insert(5u16, full);
        map.insert(60000u16, Chest::default());

        let mut buf = Vec::new();
        put_chests(&mut buf, &map);
        let mut r = Reader::new(&buf);
        let got = get_chests(&mut r).expect("decodes");
        assert_eq!(got, map, "chest state survives the round-trip");
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_chests(&mut buf, &HashMap::new());
        let mut r = Reader::new(&buf);
        assert!(get_chests(&mut r).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        let mut buf = Vec::new();
        buf.put_u16(1); // claims one chest, provides no body
        let mut r = Reader::new(&buf);
        assert!(get_chests(&mut r).is_none());
    }
}
