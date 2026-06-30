use crate::furnace::Facing;
use crate::item::ItemStack;
pub const CHEST_SLOTS: usize = 27;
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Chest {
    pub slots: [Option<ItemStack>; CHEST_SLOTS],
    pub facing: Facing,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    #[test]
    fn default_chest_is_empty_and_north() {
        let c = Chest::default();
        assert!(c.slots.iter().all(Option::is_none));
        assert_eq!(c.facing, Facing::North);
        assert_eq!(c.slots.len(), 27);
    }

    #[test]
    fn a_chest_with_any_item_is_not_empty() {
        let mut c = Chest::default();
        c.slots[13] = Some(ItemStack::new(ItemType::Stone, 1));
        assert!(c.slots.iter().any(Option::is_some));
    }
}
