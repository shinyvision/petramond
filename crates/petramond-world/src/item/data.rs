use crate::block::Block;

use super::definition::ItemDef;
use super::{load, ItemType};

pub const ENGINE_ITEM_NAMES: &[&str] = &[
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
    "petramond:pebble",
    "petramond:hemp",
    "petramond:rope",
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
    "petramond:stone_axe",
    "petramond:iron_axe",
    "petramond:diamond_axe",
    "petramond:iron_pickaxe",
    "petramond:diamond_pickaxe",
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
    "petramond:oak_fence",
    "petramond:spruce_fence",
    "petramond:birch_fence",
    "petramond:jungle_fence",
    "petramond:acacia_fence",
    "petramond:redwood_fence",
    "petramond:clay_block",
    "petramond:lava_bucket",
    "petramond:schematic_wand",
    "petramond:paper",
    "petramond:stone_bricks",
    "petramond:stone_bricks_stairs",
    "petramond:stone_bricks_slab",
    "petramond:chiseling_station",
    "petramond:oak_trapdoor",
    "petramond:spruce_trapdoor",
    "petramond:birch_trapdoor",
    "petramond:jungle_trapdoor",
    "petramond:acacia_trapdoor",
    "petramond:redwood_trapdoor",
];

pub(crate) struct ItemTables {
    defs: &'static [ItemDef],
    all: Box<[ItemType]>,
    block_to_item: Box<[ItemType]>,
    key_to_item: std::collections::HashMap<&'static str, ItemType>,
}

pub(crate) fn load_tables(
    packs: &crate::assets::PackSet,
    names: &crate::registry::ContentNames,
) -> Result<ItemTables, String> {
    let defs = load::table(packs, names)?;
    let n = Block::all().len();
    let mut block_to_item = vec![ItemType::Air; n].into_boxed_slice();
    let mut linked = vec![false; n].into_boxed_slice();
    for d in defs {
        if d.key.starts_with(super::creative::PREFIX) {
            continue;
        }
        if let Some(b) = d.block {
            if !linked[b.id() as usize] {
                block_to_item[b.id() as usize] = d.item;
                linked[b.id() as usize] = true;
            }
        }
    }
    Ok(ItemTables {
        defs,
        all: (0..defs.len()).map(|id| ItemType(id as u16)).collect(),
        block_to_item,
        key_to_item: defs.iter().map(|d| (d.key, d.item)).collect(),
    })
}

#[inline]
fn tables() -> &'static ItemTables {
    crate::content::current().items()
}

pub(super) fn all() -> &'static [ItemType] {
    &tables().all
}

#[inline]
pub(super) fn from_id(id: u16) -> ItemType {
    tables()
        .defs
        .get(id as usize)
        .map_or(ItemType::Air, |d| d.item)
}

#[inline]
pub(super) fn def(item: ItemType) -> &'static ItemDef {
    let defs = tables().defs;
    defs.get(item.id() as usize).unwrap_or(&defs[0])
}

#[inline]
pub(super) fn item_for_block(block: Block) -> ItemType {
    tables()
        .block_to_item
        .get(block.id() as usize)
        .copied()
        .unwrap_or(ItemType::Air)
}

#[inline]
pub(super) fn item_for_key(key: &str) -> Option<ItemType> {
    tables().key_to_item.get(key).copied()
}
