//! Engine-backed mod container storage plus the machine recipe read that
//! makes furnace-like machine logic possible without duplicating data.
//! (Item registry reads — `resolve_item`, `items_by_tag`, `item_info` —
//! live in [`crate::registry`].)

use mod_api::ItemStackData;

use crate::__rt::host_fn;

host_fn! {
    /// Read every slot of the mod container at `pos` — the engine-backed item
    /// storage behind a mod GUI document's `container` role slots. Multi-cell
    /// model blocks key it at the group's base cell (the `block_placed` anchor).
    /// `None` = unloaded section or no container exists there yet.
    pub fn container_get(pos: [i32; 3]) -> Option<Vec<Option<ItemStackData>>>
        => ContainerGet { pos } => ContainerSlots
}

host_fn! {
    /// Batched [`container_get`]: every listed position's slots in ONE crossing —
    /// the required shape for a machine mod's tick loop (never loop
    /// `container_get` per placed machine). At most 4096 positions per call
    /// (the sim batch cap); more disables the mod. Parallel to `positions`;
    /// `None` = unloaded or no container there yet.
    pub fn container_get_many(positions: Vec<[i32; 3]>) -> Vec<Option<Vec<Option<ItemStackData>>>>
        => ContainerGetMany { positions } => Containers
}

host_fn! {
    /// Write container slots at `pos` as `(slot index, stack)` entries (one
    /// batched call — never loop per slot). Creates/grows the container as
    /// needed; counts past an item's stack cap are clamped to it. The block at
    /// `pos` must be one of THIS mod's own registered blocks. `false` = section
    /// unloaded or an unknown item name (the batch is not applied).
    pub fn container_set(pos: [i32; 3], slots: Vec<(u32, Option<ItemStackData>)>) -> bool
        => ContainerSet { pos, slots } => Bool
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
