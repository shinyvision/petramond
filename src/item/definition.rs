//! Per-item static data (`ItemDef`).
//!
//! Mirrors `block/definition.rs`: a small POD struct in the id-ordered table
//! loaded from `assets/items.json` (see `super::load`), looked up via
//! `def(item)` / `from_id(id)`.

use crate::atlas::Tile;

use super::{HeldPose, ItemTag, ItemType};

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct ItemDef {
    pub item: ItemType,
    /// Stable snake_case identity a recipe references (e.g. `oak_planks`). This is
    /// the item's real id, independent of [`name`](Self::name).
    pub key: &'static str,
    /// Human-readable display name (UI only — not the recipe identity; see
    /// [`key`](Self::key)).
    pub name: &'static str,
    pub max_stack_size: u8,
    /// First-person hold orientation when this item is held as a sprite (see
    /// [`ItemType::held_pose`](super::ItemType::held_pose)). Most items carry
    /// [`HeldPose::DEFAULT`]; tools override it.
    pub held_pose: HeldPose,
    /// The flat atlas sprite this item draws as a billboard (slots / in-hand /
    /// dropped) — carried by the item-only items (tools, raw drops) and the
    /// block-items whose in-world model has no usable icon face (doors, the
    /// torch). `None` for cube/cross/model block-items (their icon comes from
    /// the block) and for bbmodel items (see `ItemType::item_model`).
    pub sprite: Option<Tile>,
    /// Recipe group memberships (e.g. `#planks`) this item carries — see
    /// [`ItemType::has_tag`](super::ItemType::has_tag). Most items carry none
    /// (`&[]`); a member lists each tag it belongs to.
    pub tags: &'static [ItemTag],
}
