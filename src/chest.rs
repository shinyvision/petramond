//! The chest block-entity: per-chest item storage, owned by the chunk it sits in
//! (see [`Chunk::chests`](crate::chunk::Chunk)).
//!
//! A chest holds one row-major grid of 27 item slots (3 rows × 9 columns) plus the
//! horizontal `facing` set when it was placed (which orients its model and the
//! hinge of its lid). Unlike the furnace it has no tick — it just stores items — so
//! it is pure data: place it, edit its slots through the GUI, break it to spill the
//! contents. Mirrors [`crate::furnace::Furnace`] minus the smelting machinery.

use crate::furnace::Facing;
use crate::item::ItemStack;

/// Storage slots in one chest: 3 rows × 9 columns, row-major (matching the GUI grid
/// and the player inventory's main grid).
pub const CHEST_SLOTS: usize = 27;

/// One chest's contents and placement orientation. POD and `Copy` (like
/// [`Furnace`](crate::furnace::Furnace)) so the owning chunk can store it by value.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Chest {
    /// The 27 storage slots, row-major. `None` = empty.
    pub slots: [Option<ItemStack>; CHEST_SLOTS],
    /// Which way the chest front faces (placement orientation). Rendering only:
    /// it orients the model and the lid hinge. Shares [`Facing`] with the furnace,
    /// which is where that block-orientation enum currently lives.
    pub facing: Facing,
}

impl Default for Chest {
    fn default() -> Self {
        // `[None; N]` relies on `Option<ItemStack>: Copy` (it is). Arrays have no
        // blanket `Default` impl, so this is spelled out rather than derived.
        Chest {
            slots: [None; CHEST_SLOTS],
            facing: Facing::default(),
        }
    }
}

impl Chest {
    /// `true` when every slot is empty — used when breaking the block (nothing to
    /// spill) and to prune chests that no longer need saving.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    #[test]
    fn default_chest_is_empty_and_north() {
        let c = Chest::default();
        assert!(c.is_empty());
        assert_eq!(c.facing, Facing::North);
        assert_eq!(c.slots.len(), 27);
    }

    #[test]
    fn a_chest_with_any_item_is_not_empty() {
        let mut c = Chest::default();
        c.slots[13] = Some(ItemStack::new(ItemType::Stone, 1));
        assert!(!c.is_empty());
    }
}
