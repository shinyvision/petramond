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
use crate::save::codec::{get_indexed, get_item_slot, put_indexed, put_item_slot, Reader, Writer};

/// Bytes per serialized furnace: idx(2) + 3 slots × (id 1 + count 1) + cook(2) +
/// burn(2) + burn_max(2) + facing(1).
const FURNACE_BYTES: usize = 2 + 6 + 6 + 1;

/// Append a `u16`-length-prefixed list of `(local index, furnace)` records to
/// `buf`. The list framing (count, sort-by-index, reserve) lives in
/// [`put_indexed`](crate::save::codec::put_indexed); this owns only the furnace
/// body: input/fuel/output slots, the three progress `u16`s, then facing.
pub fn put_furnaces(buf: &mut Vec<u8>, furnaces: &HashMap<u16, Furnace>) {
    put_indexed(buf, furnaces, FURNACE_BYTES, |buf, f| {
        put_item_slot(buf, f.input);
        put_item_slot(buf, f.fuel);
        put_item_slot(buf, f.output);
        buf.put_u16(f.cook_progress);
        buf.put_u16(f.burn_remaining);
        buf.put_u16(f.burn_max);
        buf.put_u8(f.facing.to_u8());
    });
}

/// Read the furnace list written by [`put_furnaces`]. `None` on truncated input.
pub fn get_furnaces(r: &mut Reader) -> Option<HashMap<u16, Furnace>> {
    get_indexed(r, |r| {
        let input = get_item_slot(r)?;
        let fuel = get_item_slot(r)?;
        let output = get_item_slot(r)?;
        let cook_progress = r.u16()?;
        let burn_remaining = r.u16()?;
        let burn_max = r.u16()?;
        let facing = Facing::from_u8(r.u8()?);
        Some(Furnace {
            input,
            fuel,
            output,
            cook_progress,
            burn_remaining,
            burn_max,
            facing,
        })
    })
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

    /// Frozen golden for the on-disk furnace framing. The roundtrip test above
    /// proves self-consistency (encode then decode), but NOT that the byte layout
    /// is unchanged -- a reframed codec could roundtrip yet read no old save. This
    /// pins the exact bytes `put_furnaces` emits for ONE fully-populated furnace
    /// (all three slots set, non-zero cook/burn/burn_max, non-default facing) so the
    /// later codec-framing extraction is proven byte-identical, not merely
    /// self-consistent.
    #[test]
    fn put_furnaces_golden_bytes() {
        let furnace = Furnace {
            input: Some(ItemStack::new(ItemType::RawIron, 30)),
            fuel: Some(ItemStack::new(ItemType::Coal, 2)),
            output: Some(ItemStack::new(ItemType::IronIngot, 7)),
            cook_progress: 123,
            burn_remaining: 456,
            burn_max: 4800,
            facing: Facing::East,
        };
        let mut map = HashMap::new();
        map.insert(0x1234u16, furnace);

        let mut buf = Vec::new();
        put_furnaces(&mut buf, &map);

        // count=1 (LE u16), idx=0x1234 (LE), input [id, 30], fuel [id, 2],
        // output [id, 7], cook=123 (LE), burn=456 (LE), burn_max=4800 (LE),
        // facing=East(3). Item ids are embedded numerically so an id renumber
        // also trips this.
        assert_eq!(
            buf.as_slice(),
            &[
                0x01,
                0x00, // count = 1
                0x34,
                0x12, // idx = 0x1234
                ItemType::RawIron.id(),
                30, // input
                ItemType::Coal.id(),
                2, // fuel
                ItemType::IronIngot.id(),
                7, // output
                123,
                0x00, // cook_progress = 123
                200,
                0x01, // burn_remaining = 456
                0xC0,
                0x12, // burn_max = 4800
                3,    // facing = East
            ][..],
            "furnace save framing changed"
        );
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
