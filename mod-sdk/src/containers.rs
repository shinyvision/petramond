//! Engine-backed mod container storage plus the machine recipe read that
//! makes furnace-like machine logic possible without duplicating data.
//! (Item registry reads — `resolve_item`, `items_by_tag`, `item_info` —
//! live in [`crate::registry`].)

use mod_api::{ContainerAddress, ItemStackData};

use crate::__rt::host_fn;

host_fn! {
    /// Read every slot of the container `at` — a block's engine-backed item
    /// storage (multi-cell model blocks key it at the group's base cell, the
    /// `block_placed` anchor) or a live mob's carried storage. A block
    /// position converts with `.into()`. `None` = unloaded, no container
    /// there yet, or no such live mob.
    pub fn container_get(at: ContainerAddress) -> Option<Vec<Option<ItemStackData>>>
        => ContainerGet { at } => ContainerSlots
}

host_fn! {
    /// Batched [`container_get`]: every listed position's slots in ONE crossing —
    /// the required shape for a machine mod's tick loop (never loop
    /// `container_get` per placed machine). At most [`crate::SIM_BATCH_MAX`] positions
    /// per call; more disables the mod. Parallel to `positions`;
    /// `None` = unloaded or no container there yet.
    pub fn container_get_many(addresses: Vec<ContainerAddress>) -> Vec<Option<Vec<Option<ItemStackData>>>>
        => ContainerGetMany { addresses } => Containers
}

host_fn! {
    /// Write container slots `at` as `(slot index, stack)` entries (one
    /// batched call — never loop per slot). A block container is created or
    /// grown as needed; a mob's never grows past its row's capacity. Counts
    /// past an item's stack cap are clamped to it. The block (or mob species)
    /// must be one of THIS mod's own. `false` = unloaded, a missing mob, a
    /// slot past a mob's capacity, or an unknown item name (the batch is not
    /// applied).
    pub fn container_set(at: ContainerAddress, slots: Vec<(u32, Option<ItemStackData>)>) -> bool
        => ContainerSet { at, slots } => Bool
}

host_fn! {
    /// The loaded machine-processing result for one input item (by registry
    /// NAME) under a recipe `class` (the same layered catalog engine machines
    /// cook from — the furnace consumes `"petramond:smelting"`; name your
    /// machine's own class and any pack can add recipes for it). `None` = no
    /// recipe.
    pub fn recipe_result(class: &str, item: &str) -> Option<ItemStackData>
        => RecipeResult { class: class.into(), item: item.into() } => ItemStack
}

host_fn! {
    /// Insert into any container using its declared admission rules, returning any remainder.
    pub fn container_insert(at: ContainerAddress, stack: ItemStackData) -> Option<ItemStackData>
        => ContainerInsert { at, stack } => ItemStack
}
host_fn! {
    /// Take up to count from one slot of any container.
    pub fn container_take(at: ContainerAddress, slot: u32, count: u8) -> Option<ItemStackData>
        => ContainerTake { at, slot, count } => ItemStack
}
host_fn! {
    /// Move up to `count` items from slot `slot` of `from` into `to` through
    /// `to`'s slot admission, as one atomic step: what `to` refuses stays in
    /// the source. Returns what actually moved.
    pub fn container_transfer(from: ContainerAddress, slot: u32, to: ContainerAddress, count: u8) -> Option<ItemStackData>
        => ContainerTransfer { from, slot, to, count } => ItemStack
}
host_fn! {
    /// Hold the container at `at` open on behalf of a live mob, or let it go:
    /// a chest's lid lifts (with its sound) while anyone holds it, as for a
    /// player's open screen. Holds end when the mob leaves the world.
    pub fn container_hold(at: ContainerAddress, actor: mod_api::EntityRef, open: bool) -> bool
        => ContainerHold { at, actor, open } => Bool
}
