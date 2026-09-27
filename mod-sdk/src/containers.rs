use mod_api::{ContainerAddress, ItemStackData};

use crate::__rt::host_fn;

host_fn! {
    pub fn container_get(at: ContainerAddress) -> Option<Vec<Option<ItemStackData>>>
        => ContainerGet { at } => ContainerSlots
}

host_fn! {
    pub fn container_get_many(addresses: Vec<ContainerAddress>) -> Vec<Option<Vec<Option<ItemStackData>>>>
        => ContainerGetMany { addresses } => Containers
}

host_fn! {
    pub fn container_set(at: ContainerAddress, slots: Vec<(u32, Option<ItemStackData>)>) -> bool
        => ContainerSet { at, slots } => Bool
}

host_fn! {
    pub fn recipe_result(class: &str, item: &str) -> Option<ItemStackData>
        => RecipeResult { class: class.into(), item: item.into() } => ItemStack
}

host_fn! {
    pub fn container_insert(at: ContainerAddress, stack: ItemStackData) -> Option<ItemStackData>
        => ContainerInsert { at, stack } => ItemStack
}
host_fn! {
    pub fn container_take(at: ContainerAddress, slot: u32, count: u8) -> Option<ItemStackData>
        => ContainerTake { at, slot, count } => ItemStack
}
host_fn! {
    pub fn container_transfer(from: ContainerAddress, slot: u32, to: ContainerAddress, count: u8) -> Option<ItemStackData>
        => ContainerTransfer { from, slot, to, count } => ItemStack
}
host_fn! {
    pub fn container_hold(at: ContainerAddress, actor: mod_api::EntityRef, open: bool) -> bool
        => ContainerHold { at, actor, open } => Bool
}
