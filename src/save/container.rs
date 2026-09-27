//! (De)serialization for the generic containers (chest, furnace, and mod
//! block slot storage alike) stored inside a section's save record.
//!
//! Rides the section record behind `FLAG2_HAS_CONTAINERS`. The slot list is
//! variable-length (a chest is 27, a furnace 3, a mod container sized by its
//! owning GUI document): the body is a `u8` slot count followed by that many
//! [`DiskSlot`]s.
//!
//! A slot whose item this build cannot resolve (a removed or disabled mod's
//! item) loads EMPTY and is kept aside, by slot index, as [`KeptSlots`];
//! the section codec stores those in the cell's KV and writes each one back
//! into its slot while the slot is still empty, so the item returns with its
//! mod. Filling the slot in the meantime replaces it.

use crate::save::codec::{get_indexed, put_indexed, put_u8, DiskSlot, Reader};
use crate::save::palette::Palette;
use crate::save::wire::Wire;
use petramond_world::container::{Container, MAX_CONTAINER_SLOTS};
use petramond_world::section::CellMap;

pub type KeptSlots = Vec<(u8, DiskSlot)>;

pub fn put_containers(
    buf: &mut Vec<u8>,
    containers: &CellMap<Container>,
    kept: &CellMap<KeptSlots>,
    pal: &Palette,
) {
    let stored: CellMap<Vec<DiskSlot>> = containers
        .iter()
        .map(|(&idx, c)| {
            debug_assert!(c.slots.len() <= MAX_CONTAINER_SLOTS);
            let kept = kept.get(&idx);
            let slots = c
                .slots
                .iter()
                .take(MAX_CONTAINER_SLOTS)
                .enumerate()
                .map(|(i, slot)| {
                    let kept_here = kept
                        .and_then(|k| k.iter().find(|(at, _)| usize::from(*at) == i))
                        .filter(|_| slot.is_none());
                    match kept_here {
                        Some((_, stored)) => stored.clone(),
                        None => DiskSlot::of(*slot, pal),
                    }
                })
                .collect();
            (idx, slots)
        })
        .collect();
    put_indexed(buf, &stored, 8, |buf, slots| {
        put_u8(buf, slots.len() as u8);
        for slot in slots {
            slot.put(buf);
        }
    });
}

pub fn get_containers(
    r: &mut Reader,
    pal: &Palette,
) -> Option<(CellMap<Container>, CellMap<KeptSlots>)> {
    let stored = get_indexed(r, |r| {
        let len = r.u8()?;
        (0..len)
            .map(|_| DiskSlot::get(r))
            .collect::<Option<Vec<_>>>()
    })?;
    let mut containers = CellMap::new();
    let mut kept = CellMap::new();
    for (idx, slots) in stored {
        let mut live = Vec::with_capacity(slots.len());
        let mut unresolved = Vec::new();
        for (i, slot) in slots.into_iter().enumerate() {
            match slot.resolve(pal) {
                Ok(slot) => live.push(slot),
                Err(stored) => {
                    live.push(None);
                    unresolved.push((i as u8, stored));
                }
            }
        }
        if !unresolved.is_empty() {
            kept.insert(idx, unresolved);
        }
        containers.insert(idx, Container { slots: live });
    }
    Some((containers, kept))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::codec::put_u16;
    use petramond_world::item::{ItemStack, ItemType};

    #[test]
    fn containers_roundtrip_through_a_buffer() {
        let mut oven = Container::with_len(3);
        oven.slots[0] = Some(ItemStack::new(ItemType::RawIron, 12));
        oven.slots[1] = Some(ItemStack::new(ItemType::Coal, 3));

        let mut map = CellMap::new();
        map.insert(7u16, oven);
        map.insert(400u16, Container::with_len(9));

        let pal = Palette::identity();
        let mut buf = Vec::new();
        put_containers(&mut buf, &map, &CellMap::new(), &pal);
        let mut r = Reader::new(&buf);
        let (got, kept) = get_containers(&mut r, &pal).expect("decodes");
        assert_eq!(got, map, "slot counts and contents survive the round-trip");
        assert!(kept.is_empty());
    }

    #[test]
    fn an_unresolvable_item_is_kept_until_its_slot_is_filled() {
        let pal = Palette::identity();
        let strange = DiskSlot {
            item: u16::MAX - 1,
            count: 5,
            blob: Default::default(),
        };
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        put_u16(&mut buf, 12);
        put_u8(&mut buf, 3);
        DiskSlot::of(Some(ItemStack::new(ItemType::Coal, 1)), &pal).put(&mut buf);
        strange.put(&mut buf);
        DiskSlot::default().put(&mut buf);

        let (containers, kept) = get_containers(&mut Reader::new(&buf), &pal).expect("decodes");
        let chest = &containers[&12];
        assert_eq!(chest.slots[1], None, "the unknown item loads as empty");
        assert_eq!(kept[&12], vec![(1, strange.clone())]);

        let mut again = Vec::new();
        put_containers(&mut again, &containers, &kept, &pal);
        assert_eq!(again, buf, "the kept item goes back into its slot");

        let mut filled = containers.clone();
        filled.get_mut(&12).unwrap().slots[1] = Some(ItemStack::new(ItemType::Stone, 2));
        let mut replaced = Vec::new();
        put_containers(&mut replaced, &filled, &kept, &pal);
        let (back, kept_again) =
            get_containers(&mut Reader::new(&replaced), &pal).expect("decodes");
        assert_eq!(back[&12].slots[1], Some(ItemStack::new(ItemType::Stone, 2)));
        assert!(
            kept_again.is_empty(),
            "filling the slot replaced the kept item"
        );
    }

    #[test]
    fn truncated_input_is_none() {
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        let mut r = Reader::new(&buf);
        assert!(get_containers(&mut r, &Palette::identity()).is_none());
    }
}
