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
use crate::save::codec::{get_indexed, get_item_slot, put_indexed, put_item_slot, Reader, Writer};

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
        buf.put_u8(c.facing.to_u8());
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

    /// Frozen golden for the on-disk chest framing. Like the furnace golden, the
    /// roundtrip test above only proves self-consistency; this pins the exact bytes
    /// `put_chests` emits for ONE fully-populated chest (several of the 27 slots
    /// filled across the row-major grid, plus a non-default facing). The 60-byte
    /// record is FNV-1a-hashed rather than spelled out byte-for-byte. Any reframing
    /// of the chest codec (slot order, length prefix, facing placement, item-id
    /// renumber) flips this.
    #[test]
    fn put_chests_golden_bytes() {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        fn fnv1a(bytes: &[u8]) -> u64 {
            let mut h = FNV_OFFSET;
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(FNV_PRIME);
            }
            h
        }

        let mut chest = Chest {
            facing: Facing::South,
            ..Chest::default()
        };
        // Fill across the grid: first/middle/last of each conceptual row.
        chest.slots[0] = Some(ItemStack::new(ItemType::Stone, 64));
        chest.slots[4] = Some(ItemStack::new(ItemType::OakLog, 12));
        chest.slots[8] = Some(ItemStack::new(ItemType::Coal, 5));
        chest.slots[13] = Some(ItemStack::new(ItemType::RawIron, 3));
        chest.slots[22] = Some(ItemStack::new(ItemType::IronIngot, 9));
        chest.slots[26] = Some(ItemStack::new(ItemType::Dirt, 1));

        let mut map = HashMap::new();
        map.insert(0x1234u16, chest);

        let mut buf = Vec::new();
        put_chests(&mut buf, &map);

        // 60-byte record: count(2) + idx(2) + 27 slots x 2 + facing(1).
        assert_eq!(buf.len(), 2 + 2 + CHEST_SLOTS * 2 + 1);
        assert_eq!(
            fnv1a(&buf),
            0x7707_5f59_bd79_86e7,
            "chest save framing changed"
        );
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
