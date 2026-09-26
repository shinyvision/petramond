//! The one shared slot codec: inventory/container/item-entity/mob slots all
//! store the same [`DiskSlot`] shape through the save palette.

use petramond_world::item::{ItemStack, ItemType};

use crate::save::palette::Palette;
use crate::save::wire::{wire_struct, Blob16};

/// One slot as stored: the world's DISK item id, the count, and the
/// instance-data blob (`[id: u16][count: u8][blob len: u16][blob]`; an empty
/// slot is all zeros). The blob is the variant's CANONICAL bytes
/// ([`petramond_world::item::variant::encode`]) — the disk never sees the
/// session [`petramond_world::item::VariantId`].
///
/// A slot whose item this build cannot resolve (unknown, or its mod is
/// disabled for the world) stays a `DiskSlot`: the codecs that can keep it
/// write it back unchanged, so the item returns with its mod.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskSlot {
    pub item: u16,
    pub count: u8,
    pub blob: Blob16,
}
wire_struct!(DiskSlot { item, count, blob });

impl DiskSlot {
    pub fn is_empty(&self) -> bool {
        self.item == 0 || self.count == 0
    }

    /// A live slot as stored through `pal`.
    pub fn of(slot: Option<ItemStack>, pal: &Palette) -> Self {
        match slot {
            Some(s) if !s.is_empty() => Self {
                item: pal.item_to_disk(s.item.id()),
                count: s.count,
                blob: Blob16(
                    petramond_world::item::variant::blob(s.variant)
                        .map(|blob| blob.to_vec())
                        .unwrap_or_default(),
                ),
            },
            _ => Self::default(),
        }
    }

    /// The live slot through `pal`: `Ok(None)` for an empty slot, `Err`
    /// with the slot itself when its item cannot be resolved. A malformed
    /// instance-data blob (a save touched by a newer/modded build) degrades
    /// to a plain stack with a warning.
    pub fn resolve(self, pal: &Palette) -> Result<Option<ItemStack>, DiskSlot> {
        if self.is_empty() {
            return Ok(None);
        }
        let Some(id) = pal.item_from_disk_known(self.item) else {
            return Err(self);
        };
        let mut stack = ItemStack::new(ItemType::from_id(id), self.count);
        if !self.blob.0.is_empty() {
            match petramond_world::item::variant::intern_blob(&self.blob.0) {
                Ok(v) => stack.variant = v,
                Err(error) => {
                    log::warn!("save slot: unreadable instance-data blob dropped: {error}")
                }
            }
        }
        Ok(Some(stack))
    }
}
