use super::variant::VariantId;
use super::{variant, ItemType, Tool};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemType,
    pub count: u8,
    pub variant: VariantId,
}

impl ItemStack {
    #[inline]
    pub fn new(item: ItemType, count: u8) -> Self {
        ItemStack {
            item,
            count: count.min(item.max_stack_size()),
            variant: VariantId::NONE,
        }
    }

    #[inline]
    pub fn restack(&self, count: u8) -> Self {
        ItemStack {
            count: count.min(self.item.max_stack_size()),
            ..*self
        }
    }

    #[inline]
    pub fn with_variant(item: ItemType, count: u8, variant: VariantId) -> Self {
        ItemStack {
            variant,
            ..ItemStack::new(item, count)
        }
    }

    #[inline]
    pub fn tool(&self) -> Option<Tool> {
        let base = self.item.tool()?;
        Some(
            match variant::value(self.variant, super::tool::TOOL_DATA_KEY) {
                Some(bytes) => base.with_override(&bytes),
                None => base,
            },
        )
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.item == ItemType::Air || self.count == 0
    }

    #[inline]
    pub fn can_stack_with(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.variant == other.variant
    }

    #[inline]
    pub fn space_left(&self) -> u8 {
        self.item.max_stack_size().saturating_sub(self.count)
    }
}
