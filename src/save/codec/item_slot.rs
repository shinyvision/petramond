//! The one shared slot codec: inventory/container/item-entity slots all
//! write the same `[item id, count, blob]` shape through the save palette.

use petramond_util::bytecodec::{put_u16, put_u8, Reader};
use petramond_world::item::{ItemStack, ItemType};

/// Encode one inventory/container slot as `[item id, count]` + a `u16`-length-
/// prefixed instance-data blob (`0` = plain stack — the ordinary case), with
/// `[0, 0, 0, 0]` for an empty or absent slot. Shared by the `level`
/// (inventory/cursor), `furnace`, and item-entity codecs so the slot format
/// lives in exactly one place. The blob is the variant's CANONICAL bytes
/// ([`petramond_world::item::variant::encode`]) — the disk never sees the session
/// [`petramond_world::item::VariantId`].
pub fn put_item_slot(buf: &mut Vec<u8>, slot: Option<ItemStack>) {
    match slot {
        Some(s) if !s.is_empty() => {
            put_u16(buf, super::palette::active().item_to_disk(s.item.id()));
            put_u8(buf, s.count);
            match petramond_world::item::variant::blob(s.variant) {
                Some(blob) => {
                    put_u16(buf, blob.len() as u16);
                    buf.extend_from_slice(&blob);
                }
                None => put_u16(buf, 0),
            }
        }
        _ => {
            put_u16(buf, 0);
            put_u8(buf, 0);
            put_u16(buf, 0);
        }
    }
}

/// Decode a slot written by [`put_item_slot`]: `None` on truncated input,
/// `Some(None)` for an empty slot, else the stack. A malformed instance-data
/// blob (a save touched by a newer/modded build) degrades to a plain stack
/// with a warning, mirroring the palette's unknown-name policy.
pub fn get_item_slot(r: &mut Reader) -> Option<Option<ItemStack>> {
    let id = r.u16()?;
    let count = r.u8()?;
    let blob_len = r.u16()? as usize;
    let blob = r.bytes(blob_len)?;
    if id == 0 || count == 0 {
        return Some(None);
    }
    let id = super::palette::active().item_from_disk(id);
    let mut stack = ItemStack::new(ItemType::from_id(id), count);
    if !blob.is_empty() {
        match petramond_world::item::variant::intern_blob(blob) {
            Some(v) => stack.variant = v,
            None => log::warn!("save slot: unreadable instance-data blob dropped"),
        }
    }
    Some(Some(stack))
}
