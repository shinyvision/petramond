//! Engine-backed mod container storage plus the item/recipe registry reads
//! that make furnace-like machine logic possible without duplicating data.

use mod_api::{HostCall, HostRet, ItemInfoData, ItemStackData};

use crate::__rt;

/// Read every slot of the mod container at `pos` — the engine-backed item
/// storage behind a mod GUI document's `container` role slots. Multi-cell
/// model blocks key it at the group's base cell (the `block_placed` anchor).
/// `None` = unloaded section or no container exists there yet.
pub fn container_get(pos: [i32; 3]) -> Option<Vec<Option<ItemStackData>>> {
    match __rt::host_call(&HostCall::ContainerGet { pos }) {
        HostRet::ContainerSlots(slots) => slots,
        other => panic!("ContainerGet returned {other:?}"),
    }
}

/// Batched [`container_get`]: every listed position's slots in ONE crossing —
/// the required shape for a machine mod's tick loop (never loop
/// `container_get` per placed machine). Parallel to `positions`; `None` =
/// unloaded or no container there yet.
pub fn container_get_many(positions: Vec<[i32; 3]>) -> Vec<Option<Vec<Option<ItemStackData>>>> {
    match __rt::host_call(&HostCall::ContainerGetMany { positions }) {
        HostRet::Containers(containers) => containers,
        other => panic!("ContainerGetMany returned {other:?}"),
    }
}

/// Write container slots at `pos` as `(slot index, stack)` entries (one
/// batched call — never loop per slot). Creates/grows the container as
/// needed; counts past an item's stack cap are clamped to it. The block at
/// `pos` must be one of THIS mod's own registered blocks. `false` = section
/// unloaded or an unknown item key (the batch is not applied).
pub fn container_set(pos: [i32; 3], slots: Vec<(u32, Option<ItemStackData>)>) -> bool {
    match __rt::host_call(&HostCall::ContainerSet { pos, slots }) {
        HostRet::Bool(ok) => ok,
        other => panic!("ContainerSet returned {other:?}"),
    }
}

/// One item's registry data (stack cap, fuel burn ticks, tags), from the same
/// rows engine mechanics read. `None` = unknown key. Registry data is
/// session-stable — cache it mod-side instead of re-asking per tick.
pub fn item_info(key: &str) -> Option<ItemInfoData> {
    match __rt::host_call(&HostCall::ItemInfo { key: key.into() }) {
        HostRet::ItemInfo(info) => info,
        other => panic!("ItemInfo returned {other:?}"),
    }
}

/// The loaded machine-processing result for one input item key under a recipe
/// `class` (the same layered catalog engine machines cook from — the furnace
/// consumes `"petramond:smelting"`; name your machine's own class and any pack can
/// add recipes for it). `None` = no recipe.
pub fn recipe_result(class: &str, key: &str) -> Option<ItemStackData> {
    match __rt::host_call(&HostCall::RecipeResult {
        class: class.into(),
        key: key.into(),
    }) {
        HostRet::ItemStack(slot) => slot,
        other => panic!("RecipeResult returned {other:?}"),
    }
}
