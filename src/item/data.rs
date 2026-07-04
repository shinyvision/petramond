//! Item table plumbing: the engine name list + the JSON-loaded table.
//!
//! The rows themselves live in `assets/items.json` (see `super::load`), so item
//! data (keys, names, stack sizes, held poses, tags, uses) is editable — and
//! moddable — without a rebuild. This module keeps only what must stay compiled
//! in: the engine item NAMES in frozen id order (index == id — the completeness
//! oracle the loader validates the file against, and the low half of the
//! runtime name table packs extend; see `crate::registry`) and the
//! lazily-loaded table the accessors read.

use std::sync::LazyLock;

use crate::block::Block;

use super::definition::ItemDef;
use super::{load, ItemType};

/// Engine item names in frozen id order (`ENGINE_ITEM_NAMES[id]` names
/// `ItemType(id)`). Append-only: save palettes identify items by these
/// ids/names. Must stay in lockstep with the consts on [`ItemType`]; the
/// shipped `items.json` covering every name keeps a typo here from going
/// unnoticed.
pub(crate) const ENGINE_ITEM_NAMES: &[&str] = &[
    "air",
    "grass",
    "dirt",
    "stone",
    "sand",
    "snow",
    "water",
    "oak_log",
    "oak_leaves",
    "spruce_log",
    "birch_log",
    "jungle_log",
    "acacia_log",
    "dark_oak_log",
    "cherry_log",
    "mangrove_log",
    "spruce_leaves",
    "birch_leaves",
    "jungle_leaves",
    "acacia_leaves",
    "dark_oak_leaves",
    "mangrove_leaves",
    "cherry_leaves",
    "azalea_leaves",
    "red_sand",
    "sandstone",
    "red_sandstone",
    "terracotta",
    "white_terracotta",
    "orange_terracotta",
    "yellow_terracotta",
    "brown_terracotta",
    "red_terracotta",
    "light_gray_terracotta",
    "podzol",
    "mycelium",
    "coarse_dirt",
    "gravel",
    "clay",
    "mud",
    "moss_block",
    "snow_block",
    "packed_ice",
    "ice",
    "calcite",
    "granite",
    "diorite",
    "andesite",
    "tuff",
    "coal_ore",
    "iron_ore",
    "copper_ore",
    "gold_ore",
    "redstone_ore",
    "lapis_ore",
    "diamond_ore",
    "emerald_ore",
    "pumpkin",
    "melon",
    "cactus",
    "short_grass",
    "fern",
    "dandelion",
    "poppy",
    "cornflower",
    "allium",
    "azure_bluet",
    "oxeye_daisy",
    "red_tulip",
    "dead_bush",
    "brown_mushroom",
    "red_mushroom",
    "cobblestone",
    "oak_planks",
    "spruce_planks",
    "birch_planks",
    "jungle_planks",
    "acacia_planks",
    "dark_oak_planks",
    "cherry_planks",
    "mangrove_planks",
    "crafting_table",
    "stick",
    "wooden_pickaxe",
    "stone_pickaxe",
    "raw_iron",
    "raw_copper",
    "coal",
    "iron_ingot",
    "copper_ingot",
    "furnace",
    "chest",
    "torch",
    "diamond",
    "lapis_lazuli",
    "raw_gold",
    "gold_ingot",
    "wooden_axe",
    "stone_axe",
    "iron_axe",
    "diamond_axe",
    "iron_pickaxe",
    "diamond_pickaxe",
    "wooden_shovel",
    "stone_shovel",
    "iron_shovel",
    "diamond_shovel",
    "furniture_workbench",
    "oak_sapling",
    "spruce_sapling",
    "birch_sapling",
    "jungle_sapling",
    "acacia_sapling",
    "dark_oak_sapling",
    "cherry_sapling",
    "oak_door",
    "spruce_door",
    "birch_door",
    "jungle_door",
    "acacia_door",
    "dark_oak_door",
    "cherry_door",
    "mangrove_door",
    "redwood_log",
    "redwood_leaves",
    "redwood_planks",
    "redwood_door",
    "oak_stairs",
    "spruce_stairs",
    "birch_stairs",
    "jungle_stairs",
    "acacia_stairs",
    "dark_oak_stairs",
    "cherry_stairs",
    "mangrove_stairs",
    "redwood_stairs",
    "cobblestone_stairs",
    "stone_stairs",
    "dirt_stairs",
    "wooden_bucket",
    "water_bucket",
    "shears",
    "wool",
    "bed_frame",
    "bed",
];

/// The JSON-loaded item table. Loads exactly once, on first access; the loader
/// panics with a precise message if the file is missing or inconsistent.
static TABLE: LazyLock<&'static [ItemDef]> = LazyLock::new(load::table);

/// Every registered item in id order (engine + pack-registered).
pub(super) fn all() -> &'static [ItemType] {
    static ALL: LazyLock<Vec<ItemType>> =
        LazyLock::new(|| (0..TABLE.len()).map(|id| ItemType(id as u8)).collect());
    &ALL
}

#[inline]
pub(super) fn from_id(id: u8) -> ItemType {
    TABLE.get(id as usize).map_or(ItemType::Air, |d| d.item)
}

#[inline]
pub(super) fn def(item: ItemType) -> &'static ItemDef {
    &TABLE[item.id() as usize]
}

/// The item whose row links it to `block` (`"block"` in `items.json`) — how a
/// pack-registered block finds its inventory item. Engine rows carry no link
/// (their mapping is the compiled prefix/match in `ItemType::from_block`), so
/// this only ever matches dynamic rows.
pub(super) fn item_for_block(block: Block) -> Option<ItemType> {
    TABLE
        .iter()
        .find(|d| d.block == Some(block))
        .map(|d| d.item)
}
