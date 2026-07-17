use super::ItemType;

/// A run of identical items occupying one inventory slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemType,
    pub count: u8,
}

impl ItemStack {
    /// A stack of `count` `item`s, clamped to the item's max stack size.
    #[inline]
    pub fn new(item: ItemType, count: u8) -> Self {
        ItemStack {
            item,
            count: count.min(item.max_stack_size()),
        }
    }

    /// `true` if this slot holds nothing (`Air` or zero count).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.item == ItemType::Air || self.count == 0
    }

    /// `true` if `other` can merge into this stack (same non-empty item type).
    #[inline]
    pub fn can_stack_with(&self, other: &ItemStack) -> bool {
        self.item == other.item
    }

    /// How many more of this item fit before hitting the max stack size.
    #[inline]
    pub fn space_left(&self) -> u8 {
        self.item.max_stack_size().saturating_sub(self.count)
    }
}
