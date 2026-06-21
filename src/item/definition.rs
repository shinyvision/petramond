//! Per-item static data (`ItemDef`).
//!
//! Mirrors `block/definition.rs`: a small POD struct stored in an id-ordered
//! table (`data::ITEM_DEFS`) and looked up via `def(item)` / `from_id(id)`.

use super::ItemType;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct ItemDef {
    pub item: ItemType,
    pub name: &'static str,
    pub max_stack_size: u8,
}
