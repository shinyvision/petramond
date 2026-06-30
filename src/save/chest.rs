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
use crate::save::codec::{get_indexed, get_item_slot, put_indexed, put_item_slot, put_u8, Reader};

/// Bytes per serialized chest: idx(2) + 27 slots × (id 1 + count 1) + facing(1).
const CHEST_BYTES: usize = 2 + CHEST_SLOTS * 2 + 1;

/// Append a `u16`-length-prefixed list of `(local index, chest)` records to `buf`.
/// The list framing (count, sort-by-index, reserve) lives in
/// [`put_indexed`](crate::save::codec::put_indexed); this owns only the chest
/// body: 27 slots in grid order followed by the facing byte.
pub fn put_chests(buf: &mut Vec<u8>, chests: &HashMap<u16, Chest>) {
    put_indexed(buf, chests, CHEST_BYTES, |buf, c| {
        for slot in c.slots {
            put_item_slot(buf, slot);
        }
        put_u8(buf, c.facing.to_u8());
    });
}

/// Read the chest list written by [`put_chests`]. `None` on truncated input.
pub fn get_chests(r: &mut Reader) -> Option<HashMap<u16, Chest>> {
    get_indexed(r, |r| {
        let mut slots = [None; CHEST_SLOTS];
        for slot in slots.iter_mut() {
            *slot = get_item_slot(r)?;
        }
        let facing = Facing::from_u8(r.u8()?);
        Some(Chest { slots, facing })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{ItemStack, ItemType};
    use crate::save::codec::put_u16;

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
        put_u16(&mut buf, 1); // claims one chest, provides no body
        let mut r = Reader::new(&buf);
        assert!(get_chests(&mut r).is_none());
    }
}
