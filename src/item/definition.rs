//! Per-item static data (`ItemDef`).
//!
//! Mirrors `block/definition.rs`: a small POD struct stored in an id-ordered
//! table (`data::ITEM_DEFS`) and looked up via `def(item)` / `from_id(id)`.

use super::{HeldPose, ItemTag, ItemType};

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct ItemDef {
    pub item: ItemType,
    /// Stable snake_case identity a recipe references (e.g. `oak_planks`). This is
    /// the item's real id — independent of [`name`](Self::name), so renaming the
    /// display string never breaks a recipe. Historically derived from `name`
    /// (`name.to_ascii_lowercase().replace(' ', "_")`); now explicit, with a test
    /// (`crafting::load::tests::registry_keys_match_display_names`) pinning every
    /// key to that historical derivation so existing recipes keep resolving.
    pub key: &'static str,
    /// Human-readable display name (UI only — not the recipe identity; see
    /// [`key`](Self::key)).
    pub name: &'static str,
    pub max_stack_size: u8,
    /// First-person hold orientation when this item is held as a sprite (see
    /// [`ItemType::held_pose`](super::ItemType::held_pose)). Most items carry
    /// [`HeldPose::DEFAULT`]; tools override it.
    pub held_pose: HeldPose,
    /// Recipe group memberships (e.g. `#planks`) this item carries — see
    /// [`ItemType::has_tag`](super::ItemType::has_tag). Most items carry none
    /// (`&[]`); a member lists each tag it belongs to.
    pub tags: &'static [ItemTag],
}
