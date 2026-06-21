//! Id-ordered `ItemDef` table.
//!
//! Mirrors `block/data.rs` exactly: one row per `ItemType` variant in `id()`
//! order (which is identical to `Block` order — both start at `Air = 0`), so
//! `ItemType::from_block(b) == from_id(b.id())` and `as_block` is the inverse.
//! Append-only: never reorder or remove rows (item ids are persisted-adjacent
//! to block ids and are covered by a stability test).

use super::definition::ItemDef;
use super::ItemType;

/// Default stack size for every 0.1 block-item.
const STACK: u8 = 64;

pub(super) const ALL_ITEMS: &[ItemType] = &[
    ItemType::Air,
    ItemType::Grass,
    ItemType::Dirt,
    ItemType::Stone,
    ItemType::Sand,
    ItemType::Snow,
    ItemType::Water,
    ItemType::OakLog,
    ItemType::OakLeaves,
    ItemType::SpruceLog,
    ItemType::BirchLog,
    ItemType::JungleLog,
    ItemType::AcaciaLog,
    ItemType::DarkOakLog,
    ItemType::CherryLog,
    ItemType::MangroveLog,
    ItemType::SpruceLeaves,
    ItemType::BirchLeaves,
    ItemType::JungleLeaves,
    ItemType::AcaciaLeaves,
    ItemType::DarkOakLeaves,
    ItemType::MangroveLeaves,
    ItemType::CherryLeaves,
    ItemType::AzaleaLeaves,
    ItemType::RedSand,
    ItemType::Sandstone,
    ItemType::RedSandstone,
    ItemType::Terracotta,
    ItemType::WhiteTerracotta,
    ItemType::OrangeTerracotta,
    ItemType::YellowTerracotta,
    ItemType::BrownTerracotta,
    ItemType::RedTerracotta,
    ItemType::LightGrayTerracotta,
    ItemType::Podzol,
    ItemType::Mycelium,
    ItemType::CoarseDirt,
    ItemType::Gravel,
    ItemType::Clay,
    ItemType::Mud,
    ItemType::MossBlock,
    ItemType::SnowBlock,
    ItemType::PackedIce,
    ItemType::Ice,
    ItemType::Calcite,
    ItemType::Granite,
    ItemType::Diorite,
    ItemType::Andesite,
    ItemType::Tuff,
    ItemType::CoalOre,
    ItemType::IronOre,
    ItemType::CopperOre,
    ItemType::GoldOre,
    ItemType::RedstoneOre,
    ItemType::LapisOre,
    ItemType::DiamondOre,
    ItemType::EmeraldOre,
    ItemType::Pumpkin,
    ItemType::Melon,
    ItemType::Cactus,
    ItemType::ShortGrass,
    ItemType::Fern,
    ItemType::Dandelion,
    ItemType::Poppy,
    ItemType::Cornflower,
    ItemType::Allium,
    ItemType::AzureBluet,
    ItemType::OxeyeDaisy,
    ItemType::RedTulip,
    ItemType::DeadBush,
    ItemType::BrownMushroom,
    ItemType::RedMushroom,
];

pub(super) const ITEM_DEFS: &[ItemDef] = &[
    ItemDef {
        item: ItemType::Air,
        name: "Air",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Grass,
        name: "Grass Block",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Dirt,
        name: "Dirt",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Stone,
        name: "Stone",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Sand,
        name: "Sand",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Snow,
        name: "Snow",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Water,
        name: "Water",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::OakLog,
        name: "Oak Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::OakLeaves,
        name: "Oak Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::SpruceLog,
        name: "Spruce Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::BirchLog,
        name: "Birch Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::JungleLog,
        name: "Jungle Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::AcaciaLog,
        name: "Acacia Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::DarkOakLog,
        name: "Dark Oak Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::CherryLog,
        name: "Cherry Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::MangroveLog,
        name: "Mangrove Log",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::SpruceLeaves,
        name: "Spruce Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::BirchLeaves,
        name: "Birch Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::JungleLeaves,
        name: "Jungle Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::AcaciaLeaves,
        name: "Acacia Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::DarkOakLeaves,
        name: "Dark Oak Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::MangroveLeaves,
        name: "Mangrove Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::CherryLeaves,
        name: "Cherry Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::AzaleaLeaves,
        name: "Azalea Leaves",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::RedSand,
        name: "Red Sand",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Sandstone,
        name: "Sandstone",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::RedSandstone,
        name: "Red Sandstone",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Terracotta,
        name: "Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::WhiteTerracotta,
        name: "White Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::OrangeTerracotta,
        name: "Orange Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::YellowTerracotta,
        name: "Yellow Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::BrownTerracotta,
        name: "Brown Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::RedTerracotta,
        name: "Red Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::LightGrayTerracotta,
        name: "Light Gray Terracotta",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Podzol,
        name: "Podzol",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Mycelium,
        name: "Mycelium",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::CoarseDirt,
        name: "Coarse Dirt",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Gravel,
        name: "Gravel",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Clay,
        name: "Clay",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Mud,
        name: "Mud",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::MossBlock,
        name: "Moss Block",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::SnowBlock,
        name: "Snow Block",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::PackedIce,
        name: "Packed Ice",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Ice,
        name: "Ice",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Calcite,
        name: "Calcite",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Granite,
        name: "Granite",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Diorite,
        name: "Diorite",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Andesite,
        name: "Andesite",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Tuff,
        name: "Tuff",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::CoalOre,
        name: "Coal Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::IronOre,
        name: "Iron Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::CopperOre,
        name: "Copper Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::GoldOre,
        name: "Gold Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::RedstoneOre,
        name: "Redstone Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::LapisOre,
        name: "Lapis Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::DiamondOre,
        name: "Diamond Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::EmeraldOre,
        name: "Emerald Ore",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Pumpkin,
        name: "Pumpkin",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Melon,
        name: "Melon",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Cactus,
        name: "Cactus",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::ShortGrass,
        name: "Short Grass",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Fern,
        name: "Fern",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Dandelion,
        name: "Dandelion",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Poppy,
        name: "Poppy",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Cornflower,
        name: "Cornflower",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::Allium,
        name: "Allium",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::AzureBluet,
        name: "Azure Bluet",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::OxeyeDaisy,
        name: "Oxeye Daisy",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::RedTulip,
        name: "Red Tulip",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::DeadBush,
        name: "Dead Bush",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::BrownMushroom,
        name: "Brown Mushroom",
        max_stack_size: STACK,
    },
    ItemDef {
        item: ItemType::RedMushroom,
        name: "Red Mushroom",
        max_stack_size: STACK,
    },
];

#[inline]
pub(super) fn from_id(id: u8) -> ItemType {
    ITEM_DEFS
        .get(id as usize)
        .map_or(ItemType::Air, |def| def.item)
}

#[inline]
pub(super) fn def(item: ItemType) -> &'static ItemDef {
    let index = item.id() as usize;
    debug_assert!(
        index < ITEM_DEFS.len() && ITEM_DEFS[index].item == item,
        "ITEM_DEFS must be ordered by ItemType::id()"
    );
    &ITEM_DEFS[index]
}
