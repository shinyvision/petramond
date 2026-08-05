use super::variant::VariantId;
use super::{variant, ItemType, Tool};

/// A run of identical items occupying one inventory slot. Identity for
/// stacking is (`item`, `variant`): instance-data-bearing stacks merge only
/// with byte-identical data (see [`super::variant`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemType,
    pub count: u8,
    pub variant: VariantId,
}

impl ItemStack {
    /// A stack of `count` plain `item`s, clamped to the item's max stack size.
    #[inline]
    pub fn new(item: ItemType, count: u8) -> Self {
        ItemStack {
            item,
            count: count.min(item.max_stack_size()),
            variant: VariantId::NONE,
        }
    }

    /// This stack's identity with a different count — the split/rebuild
    /// constructor: it preserves the variant, which `new` deliberately drops.
    #[inline]
    pub fn restack(&self, count: u8) -> Self {
        ItemStack {
            count: count.min(self.item.max_stack_size()),
            ..*self
        }
    }

    /// A stack carrying instance data.
    #[inline]
    pub fn with_variant(item: ItemType, count: u8, variant: VariantId) -> Self {
        ItemStack {
            variant,
            ..ItemStack::new(item, count)
        }
    }

    /// This STACK as a mining [`Tool`]: the row-resolved
    /// [`ItemType::tool`](super::ItemType::tool) with any
    /// [`TOOL_DATA_KEY`](super::tool::TOOL_DATA_KEY) instance-data override
    /// merged over it. Every gameplay consumer of a HELD tool (mining speed,
    /// harvest gate, attack damage) must resolve through this, never through
    /// the bare item — a bare-item read silently ignores augments.
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

    /// `true` if this slot holds nothing (`Air` or zero count).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.item == ItemType::Air || self.count == 0
    }

    /// `true` if `other` can merge into this stack (same non-empty item type
    /// AND same instance-data variant).
    #[inline]
    pub fn can_stack_with(&self, other: &ItemStack) -> bool {
        self.item == other.item && self.variant == other.variant
    }

    /// How many more of this item fit before hitting the max stack size.
    #[inline]
    pub fn space_left(&self) -> u8 {
        self.item.max_stack_size().saturating_sub(self.count)
    }
}
