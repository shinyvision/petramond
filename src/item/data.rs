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
    "llama:air",
    "llama:grass",
    "llama:dirt",
    "llama:stone",
    "llama:sand",
    "llama:snow",
    "llama:water",
    "llama:oak_log",
    "llama:oak_leaves",
    "llama:spruce_log",
    "llama:birch_log",
    "llama:jungle_log",
    "llama:acacia_log",
    "llama:dark_oak_log",
    "llama:cherry_log",
    "llama:mangrove_log",
    "llama:spruce_leaves",
    "llama:birch_leaves",
    "llama:jungle_leaves",
    "llama:acacia_leaves",
    "llama:dark_oak_leaves",
    "llama:mangrove_leaves",
    "llama:cherry_leaves",
    "llama:azalea_leaves",
    "llama:red_sand",
    "llama:sandstone",
    "llama:red_sandstone",
    "llama:terracotta",
    "llama:white_terracotta",
    "llama:orange_terracotta",
    "llama:yellow_terracotta",
    "llama:brown_terracotta",
    "llama:red_terracotta",
    "llama:light_gray_terracotta",
    "llama:podzol",
    "llama:mycelium",
    "llama:coarse_dirt",
    "llama:gravel",
    "llama:clay",
    "llama:mud",
    "llama:moss_block",
    "llama:snow_block",
    "llama:packed_ice",
    "llama:ice",
    "llama:calcite",
    "llama:granite",
    "llama:diorite",
    "llama:andesite",
    "llama:tuff",
    "llama:coal_ore",
    "llama:iron_ore",
    "llama:copper_ore",
    "llama:gold_ore",
    "llama:redstone_ore",
    "llama:lapis_ore",
    "llama:diamond_ore",
    "llama:emerald_ore",
    "llama:pumpkin",
    "llama:melon",
    "llama:cactus",
    "llama:short_grass",
    "llama:fern",
    "llama:dandelion",
    "llama:poppy",
    "llama:cornflower",
    "llama:allium",
    "llama:azure_bluet",
    "llama:oxeye_daisy",
    "llama:red_tulip",
    "llama:dead_bush",
    "llama:brown_mushroom",
    "llama:red_mushroom",
    "llama:cobblestone",
    "llama:oak_planks",
    "llama:spruce_planks",
    "llama:birch_planks",
    "llama:jungle_planks",
    "llama:acacia_planks",
    "llama:dark_oak_planks",
    "llama:cherry_planks",
    "llama:mangrove_planks",
    "llama:crafting_table",
    "llama:stick",
    "llama:wooden_pickaxe",
    "llama:stone_pickaxe",
    "llama:raw_iron",
    "llama:raw_copper",
    "llama:coal",
    "llama:iron_ingot",
    "llama:copper_ingot",
    "llama:furnace",
    "llama:chest",
    "llama:torch",
    "llama:diamond",
    "llama:lapis_lazuli",
    "llama:raw_gold",
    "llama:gold_ingot",
    "llama:wooden_axe",
    "llama:stone_axe",
    "llama:iron_axe",
    "llama:diamond_axe",
    "llama:iron_pickaxe",
    "llama:diamond_pickaxe",
    "llama:wooden_shovel",
    "llama:stone_shovel",
    "llama:iron_shovel",
    "llama:diamond_shovel",
    "llama:furniture_workbench",
    "llama:oak_sapling",
    "llama:spruce_sapling",
    "llama:birch_sapling",
    "llama:jungle_sapling",
    "llama:acacia_sapling",
    "llama:dark_oak_sapling",
    "llama:cherry_sapling",
    "llama:oak_door",
    "llama:spruce_door",
    "llama:birch_door",
    "llama:jungle_door",
    "llama:acacia_door",
    "llama:dark_oak_door",
    "llama:cherry_door",
    "llama:mangrove_door",
    "llama:redwood_log",
    "llama:redwood_leaves",
    "llama:redwood_planks",
    "llama:redwood_door",
    "llama:oak_stairs",
    "llama:spruce_stairs",
    "llama:birch_stairs",
    "llama:jungle_stairs",
    "llama:acacia_stairs",
    "llama:dark_oak_stairs",
    "llama:cherry_stairs",
    "llama:mangrove_stairs",
    "llama:redwood_stairs",
    "llama:cobblestone_stairs",
    "llama:stone_stairs",
    "llama:dirt_stairs",
    "llama:wooden_bucket",
    "llama:water_bucket",
    "llama:shears",
    "llama:wool",
    "llama:bed_frame",
    "llama:bed",
    "llama:oak_slab",
    "llama:spruce_slab",
    "llama:birch_slab",
    "llama:jungle_slab",
    "llama:acacia_slab",
    "llama:dark_oak_slab",
    "llama:cherry_slab",
    "llama:mangrove_slab",
    "llama:redwood_slab",
    "llama:cobblestone_slab",
    "llama:stone_slab",
    "llama:dirt_slab",
    "llama:glass",
    "llama:glass_pane",
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
