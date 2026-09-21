//! What the golem carries: fetching the next batch of blocks and the tools
//! the clearance ahead wants from the table's chests, choosing a tool per
//! block, and taking back salvage and leftovers.

mod chest;
mod trip;
mod wants;

pub use chest::{open, rummage};
pub use trip::{resupply, returns_anything, unload, unload_all};
pub use wants::{tool_slot, tools_waiting};

use std::collections::BTreeMap;

use mod_sdk::*;

use crate::survey::{key_of, ItemKey};

pub fn totals(slots: &[Option<ItemStackData>]) -> BTreeMap<ItemKey, u32> {
    let mut totals = BTreeMap::new();
    crate::supplies::add_totals(&mut totals, slots);
    totals
}

pub fn holds(carried: &BTreeMap<ItemKey, u32>, missing: &[ItemStackData]) -> bool {
    let mut need: BTreeMap<ItemKey, u32> = BTreeMap::new();
    for stack in missing {
        *need.entry(key_of(stack)).or_default() += u32::from(stack.count);
    }
    need.iter()
        .all(|(k, n)| carried.get(k).copied().unwrap_or(0) >= *n)
}
