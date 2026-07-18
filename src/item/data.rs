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
    "petramond:air",
    "petramond:grass",
    "petramond:dirt",
    "petramond:stone",
    "petramond:sand",
    "petramond:snowball",
    "petramond:water",
    "petramond:oak_log",
    "petramond:oak_leaves",
    "petramond:spruce_log",
    "petramond:birch_log",
    "petramond:jungle_log",
    "petramond:acacia_log",
    "petramond:spruce_leaves",
    "petramond:birch_leaves",
    "petramond:jungle_leaves",
    "petramond:acacia_leaves",
    "petramond:azalea_leaves",
    "petramond:red_sand",
    "petramond:sandstone",
    "petramond:red_sandstone",
    "petramond:terracotta",
    "petramond:white_terracotta",
    "petramond:orange_terracotta",
    "petramond:yellow_terracotta",
    "petramond:brown_terracotta",
    "petramond:red_terracotta",
    "petramond:light_gray_terracotta",
    "petramond:podzol",
    "petramond:mycelium",
    "petramond:coarse_dirt",
    "petramond:gravel",
    "petramond:clay",
    "petramond:mud",
    "petramond:moss_block",
    "petramond:snow_block",
    "petramond:packed_ice",
    "petramond:ice",
    "petramond:calcite",
    "petramond:marble",
    "petramond:tuff",
    "petramond:coal_ore",
    "petramond:iron_ore",
    "petramond:copper_ore",
    "petramond:gold_ore",
    "petramond:diamond_ore",
    "petramond:pumpkin",
    "petramond:melon",
    "petramond:cactus",
    "petramond:short_grass",
    "petramond:fern",
    "petramond:dandelion",
    "petramond:poppy",
    "petramond:cornflower",
    "petramond:allium",
    "petramond:azure_bluet",
    "petramond:oxeye_daisy",
    "petramond:red_tulip",
    "petramond:dead_bush",
    "petramond:brown_mushroom",
    "petramond:red_mushroom",
    "petramond:cobblestone",
    "petramond:oak_planks",
    "petramond:spruce_planks",
    "petramond:birch_planks",
    "petramond:jungle_planks",
    "petramond:acacia_planks",
    "petramond:crafting_table",
    "petramond:stick",
    "petramond:wooden_pickaxe",
    "petramond:stone_pickaxe",
    "petramond:raw_iron",
    "petramond:raw_copper",
    "petramond:coal",
    "petramond:iron_ingot",
    "petramond:copper_ingot",
    "petramond:furnace",
    "petramond:chest",
    "petramond:torch",
    "petramond:diamond",
    "petramond:raw_gold",
    "petramond:gold_ingot",
    "petramond:wooden_axe",
    "petramond:stone_axe",
    "petramond:iron_axe",
    "petramond:diamond_axe",
    "petramond:iron_pickaxe",
    "petramond:diamond_pickaxe",
    "petramond:wooden_shovel",
    "petramond:stone_shovel",
    "petramond:iron_shovel",
    "petramond:diamond_shovel",
    "petramond:furniture_workbench",
    "petramond:oak_sapling",
    "petramond:spruce_sapling",
    "petramond:birch_sapling",
    "petramond:jungle_sapling",
    "petramond:acacia_sapling",
    "petramond:oak_door",
    "petramond:spruce_door",
    "petramond:birch_door",
    "petramond:jungle_door",
    "petramond:acacia_door",
    "petramond:redwood_log",
    "petramond:redwood_leaves",
    "petramond:redwood_planks",
    "petramond:redwood_door",
    "petramond:oak_stairs",
    "petramond:spruce_stairs",
    "petramond:birch_stairs",
    "petramond:jungle_stairs",
    "petramond:acacia_stairs",
    "petramond:redwood_stairs",
    "petramond:cobblestone_stairs",
    "petramond:stone_stairs",
    "petramond:dirt_stairs",
    "petramond:wooden_bucket",
    "petramond:water_bucket",
    "petramond:shears",
    "petramond:wool",
    "petramond:bed_frame",
    "petramond:bed",
    "petramond:oak_slab",
    "petramond:spruce_slab",
    "petramond:birch_slab",
    "petramond:jungle_slab",
    "petramond:acacia_slab",
    "petramond:redwood_slab",
    "petramond:cobblestone_slab",
    "petramond:stone_slab",
    "petramond:dirt_slab",
    "petramond:glass",
    "petramond:glass_pane",
    "petramond:wool_block",
    "petramond:wool_stairs",
    "petramond:wool_slab",
    "petramond:polished_marble",
    "petramond:marble_stairs",
    "petramond:marble_slab",
    "petramond:polished_marble_stairs",
    "petramond:polished_marble_slab",
    "petramond:ladder",
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

/// Dense block-id → item LUT, inverted once from the rows' `block` links
/// (`"block"` in `items.json`). A block no row links to maps to `Air`
/// (nothing to hold); if several rows link one block, the lowest item id
/// wins (the growth-stage pattern: only the planting item links the stage-0
/// block, later stages link nothing).
static BLOCK_TO_ITEM: LazyLock<[ItemType; 256]> = LazyLock::new(|| {
    let mut lut = [ItemType::Air; 256];
    let mut set = [false; 256];
    for d in TABLE.iter() {
        if let Some(b) = d.block {
            if !set[b.id() as usize] {
                lut[b.id() as usize] = d.item;
                set[b.id() as usize] = true;
            }
        }
    }
    lut
});

/// The item whose row links it to `block`, or `Air` if none does.
#[inline]
pub(super) fn item_for_block(block: Block) -> ItemType {
    BLOCK_TO_ITEM[block.id() as usize]
}

/// Keyed hash index over the rows' recipe `key`s (unique — the loader
/// enforces it), built once with the table. The engine-internal keyed lookup
/// (recipes, loot tables); mod-facing identity is the registry NAME.
static KEY_TO_ITEM: LazyLock<std::collections::HashMap<&'static str, ItemType>> =
    LazyLock::new(|| TABLE.iter().map(|d| (d.key, d.item)).collect());

/// The item whose row carries recipe `key`, or `None`.
#[inline]
pub(super) fn item_for_key(key: &str) -> Option<ItemType> {
    KEY_TO_ITEM.get(key).copied()
}
