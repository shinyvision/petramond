use petramond_world::item::{ItemStack, ItemType};

use crate::save::palette::Palette;
use crate::save::wire::{wire_struct, Blob16};

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
