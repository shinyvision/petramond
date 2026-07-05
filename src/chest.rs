use crate::furnace::Facing;
use crate::item::ItemStack;
pub const CHEST_SLOTS: usize = 27;
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Chest {
    pub slots: [Option<ItemStack>; CHEST_SLOTS],
    pub facing: Facing,
}
