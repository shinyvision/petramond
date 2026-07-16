//! Engine-backed mod container storage plus the item/recipe registry reads
//! that make furnace-like machine logic possible without duplicating data.

use mod_api::{ItemId, ItemInfoData, ItemStackData};

use crate::__rt::host_fn;

host_fn! {
    /// Resolve an item registry name to this session's numeric [`ItemId`], or
    /// `None` for an unknown name. Registry-only (the [`resolve_block`] contract):
    /// legal on any instance, any time. Resolve once in `init` and compare against
    /// the ids in event payloads (`item_use_pre`) — never persist numeric ids.
    ///
    /// [`resolve_block`]: crate::worldgen::resolve_block
    pub fn resolve_item(key: &str) -> Option<ItemId> => ResolveItem { key: key.into() } => Item
}

/// [`resolve_item`] that also logs a "not registered" line on `None` — the
/// standard init-time shape: resolution failure is worth one log line, then
/// the mod degrades on the `None`.
pub fn resolve_item_logged(key: &str) -> Option<ItemId> {
    let id = resolve_item(key);
    if id.is_none() {
        crate::log(&format!("item '{key}' is not registered"));
    }
    id
}

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
    /// `container_get` per placed machine). Parallel to `positions`; `None` =
    /// unloaded or no container there yet.
    pub fn container_get_many(positions: Vec<[i32; 3]>) -> Vec<Option<Vec<Option<ItemStackData>>>>
        => ContainerGetMany { positions } => Containers
}

host_fn! {
    /// Write container slots at `pos` as `(slot index, stack)` entries (one
    /// batched call — never loop per slot). Creates/grows the container as
    /// needed; counts past an item's stack cap are clamped to it. The block at
    /// `pos` must be one of THIS mod's own registered blocks. `false` = section
    /// unloaded or an unknown item key (the batch is not applied).
    pub fn container_set(pos: [i32; 3], slots: Vec<(u32, Option<ItemStackData>)>) -> bool
        => ContainerSet { pos, slots } => Bool
}

host_fn! {
    /// One item's registry data (stack cap, fuel burn ticks, tags), from the same
    /// rows engine mechanics read. `None` = unknown key. Registry data is
    /// session-stable — cache it mod-side instead of re-asking per tick.
    pub fn item_info(key: &str) -> Option<ItemInfoData> => ItemInfo { key: key.into() } => ItemInfo
}

host_fn! {
    /// The loaded machine-processing result for one input item key under a recipe
    /// `class` (the same layered catalog engine machines cook from — the furnace
    /// consumes `"petramond:smelting"`; name your machine's own class and any pack can
    /// add recipes for it). `None` = no recipe.
    pub fn recipe_result(class: &str, key: &str) -> Option<ItemStackData>
        => RecipeResult { class: class.into(), key: key.into() } => ItemStack
}
