//! (De)serialization for the generic containers (chest, furnace, and mod
//! block slot storage alike) stored inside a chunk's save record.
//!
//! Rides the section record behind `FLAG2_HAS_CONTAINERS`. The slot list is
//! variable-length (a chest is 27, a furnace 3, a mod container sized by its
//! owning GUI document): the body is a `u8` slot count followed by that many
//! palette-mapped item slots. Items from an absent/disabled mod decode to
//! empty via the palette's unknown-name rule, like every other persisted
//! stack.

use std::collections::HashMap;

use crate::container::{Container, MAX_CONTAINER_SLOTS};
use crate::save::codec::{get_indexed, get_item_slot, put_indexed, put_item_slot, put_u8, Reader};

/// Append a `u16`-length-prefixed list of `(local index, container)` records.
pub fn put_containers(buf: &mut Vec<u8>, containers: &HashMap<u16, Container>) {
    put_indexed(buf, containers, 8, |buf, c| {
        debug_assert!(c.slots.len() <= MAX_CONTAINER_SLOTS);
        put_u8(buf, c.slots.len().min(MAX_CONTAINER_SLOTS) as u8);
        for slot in c.slots.iter().take(MAX_CONTAINER_SLOTS) {
            put_item_slot(buf, *slot);
        }
    });
}

/// Read the list written by [`put_containers`]. `None` on truncated input.
pub fn get_containers(r: &mut Reader) -> Option<HashMap<u16, Container>> {
    get_indexed(r, |r| {
        let len = r.u8()? as usize;
        let mut slots = Vec::with_capacity(len);
        for _ in 0..len {
            slots.push(get_item_slot(r)?);
        }
        Some(Container { slots })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{ItemStack, ItemType};
    use crate::save::codec::put_u16;

    #[test]
    fn containers_roundtrip_through_a_buffer() {
        let mut oven = Container::with_len(3);
        oven.slots[0] = Some(ItemStack::new(ItemType::RawIron, 12));
        oven.slots[1] = Some(ItemStack::new(ItemType::Coal, 3));

        let mut map = HashMap::new();
        map.insert(7u16, oven);
        map.insert(400u16, Container::with_len(9));

        let mut buf = Vec::new();
        put_containers(&mut buf, &map);
        let mut r = Reader::new(&buf);
        let got = get_containers(&mut r).expect("decodes");
        assert_eq!(got, map, "slot counts and contents survive the round-trip");
    }

    #[test]
    fn truncated_input_is_none() {
        let mut buf = Vec::new();
        put_u16(&mut buf, 1); // claims one container, provides no body
        let mut r = Reader::new(&buf);
        assert!(get_containers(&mut r).is_none());
    }
}
