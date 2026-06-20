use crate::atlas::Tile;

use super::definition::{BlockDef, BlockFlags};
use super::Block;

const AIR_FLAGS: BlockFlags = BlockFlags::REPLACEABLE;
const FULL_CUBE_FLAGS: BlockFlags = BlockFlags::SOLID
    .with(BlockFlags::OPAQUE)
    .with(BlockFlags::AO_OCCLUDER);
const WATER_FLAGS: BlockFlags = BlockFlags::TRANSPARENT.with(BlockFlags::REPLACEABLE);
const LEAVES_FLAGS: BlockFlags = BlockFlags::SOLID
    .with(BlockFlags::AO_OCCLUDER)
    .with(BlockFlags::TRANSPARENT);
// Cross-model plants (grass, ferns, flowers, mushrooms): transparent cutout only.
// NOT solid (walk-through, not a build target), NOT opaque (neighbour cube faces
// still draw toward them and they don't cull), NOT an AO occluder, NOT replaceable.
const PLANT_FLAGS: BlockFlags = BlockFlags::TRANSPARENT;

pub(super) const ALL_BLOCKS: &[Block] = &[
    Block::Air,
    Block::Grass,
    Block::Dirt,
    Block::Stone,
    Block::Sand,
    Block::Snow,
    Block::Water,
    Block::OakLog,
    Block::OakLeaves,
    Block::SpruceLog,
    Block::BirchLog,
    Block::JungleLog,
    Block::AcaciaLog,
    Block::DarkOakLog,
    Block::CherryLog,
    Block::MangroveLog,
    Block::SpruceLeaves,
    Block::BirchLeaves,
    Block::JungleLeaves,
    Block::AcaciaLeaves,
    Block::DarkOakLeaves,
    Block::MangroveLeaves,
    Block::CherryLeaves,
    Block::AzaleaLeaves,
    Block::RedSand,
    Block::Sandstone,
    Block::RedSandstone,
    Block::Terracotta,
    Block::WhiteTerracotta,
    Block::OrangeTerracotta,
    Block::YellowTerracotta,
    Block::BrownTerracotta,
    Block::RedTerracotta,
    Block::LightGrayTerracotta,
    Block::Podzol,
    Block::Mycelium,
    Block::CoarseDirt,
    Block::Gravel,
    Block::Clay,
    Block::Mud,
    Block::MossBlock,
    Block::SnowBlock,
    Block::PackedIce,
    Block::Ice,
    Block::Calcite,
    Block::Granite,
    Block::Diorite,
    Block::Andesite,
    Block::Tuff,
    Block::CoalOre,
    Block::IronOre,
    Block::CopperOre,
    Block::GoldOre,
    Block::RedstoneOre,
    Block::LapisOre,
    Block::DiamondOre,
    Block::EmeraldOre,
    Block::Pumpkin,
    Block::Melon,
    Block::Cactus,
    Block::ShortGrass,
    Block::Fern,
    Block::Dandelion,
    Block::Poppy,
    Block::Cornflower,
    Block::Allium,
    Block::AzureBluet,
    Block::OxeyeDaisy,
    Block::RedTulip,
    Block::DeadBush,
    Block::BrownMushroom,
    Block::RedMushroom,
];

pub(super) const BLOCK_DEFS: &[BlockDef] = &[
    BlockDef {
        block: Block::Air,
        flags: AIR_FLAGS,
        tiles: [Tile::OakLeaves, Tile::OakLeaves, Tile::OakLeaves],
    },
    BlockDef {
        block: Block::Grass,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::GrassTop, Tile::Dirt, Tile::GrassSide],
    },
    BlockDef {
        block: Block::Dirt,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Dirt, Tile::Dirt, Tile::Dirt],
    },
    BlockDef {
        block: Block::Stone,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Stone, Tile::Stone, Tile::Stone],
    },
    BlockDef {
        block: Block::Sand,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Sand, Tile::Sand, Tile::Sand],
    },
    BlockDef {
        block: Block::Snow,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Snow, Tile::Dirt, Tile::GrassSnow],
    },
    BlockDef {
        block: Block::Water,
        flags: WATER_FLAGS,
        tiles: [Tile::Water, Tile::Water, Tile::Water],
    },
    BlockDef {
        block: Block::OakLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::OakLogTop, Tile::OakLogTop, Tile::OakLogSide],
    },
    BlockDef {
        block: Block::OakLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::OakLeaves, Tile::OakLeaves, Tile::OakLeaves],
    },
    BlockDef {
        block: Block::SpruceLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::SpruceLogTop, Tile::SpruceLogTop, Tile::SpruceLogSide],
    },
    BlockDef {
        block: Block::BirchLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::BirchLogTop, Tile::BirchLogTop, Tile::BirchLogSide],
    },
    BlockDef {
        block: Block::JungleLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::JungleLogTop, Tile::JungleLogTop, Tile::JungleLogSide],
    },
    BlockDef {
        block: Block::AcaciaLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::AcaciaLogTop, Tile::AcaciaLogTop, Tile::AcaciaLogSide],
    },
    BlockDef {
        block: Block::DarkOakLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::DarkOakLogTop, Tile::DarkOakLogTop, Tile::DarkOakLogSide],
    },
    BlockDef {
        block: Block::CherryLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::CherryLogTop, Tile::CherryLogTop, Tile::CherryLogSide],
    },
    BlockDef {
        block: Block::MangroveLog,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::MangroveLogTop, Tile::MangroveLogTop, Tile::MangroveLogSide],
    },
    BlockDef {
        block: Block::SpruceLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::SpruceLeaves, Tile::SpruceLeaves, Tile::SpruceLeaves],
    },
    BlockDef {
        block: Block::BirchLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::BirchLeaves, Tile::BirchLeaves, Tile::BirchLeaves],
    },
    BlockDef {
        block: Block::JungleLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::JungleLeaves, Tile::JungleLeaves, Tile::JungleLeaves],
    },
    BlockDef {
        block: Block::AcaciaLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::AcaciaLeaves, Tile::AcaciaLeaves, Tile::AcaciaLeaves],
    },
    BlockDef {
        block: Block::DarkOakLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::DarkOakLeaves, Tile::DarkOakLeaves, Tile::DarkOakLeaves],
    },
    BlockDef {
        block: Block::MangroveLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::MangroveLeaves, Tile::MangroveLeaves, Tile::MangroveLeaves],
    },
    BlockDef {
        block: Block::CherryLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::CherryLeaves, Tile::CherryLeaves, Tile::CherryLeaves],
    },
    BlockDef {
        block: Block::AzaleaLeaves,
        flags: LEAVES_FLAGS,
        tiles: [Tile::AzaleaLeaves, Tile::AzaleaLeaves, Tile::AzaleaLeaves],
    },
    BlockDef {
        block: Block::RedSand,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::RedSand, Tile::RedSand, Tile::RedSand],
    },
    BlockDef {
        block: Block::Sandstone,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::SandstoneTop, Tile::SandstoneBottom, Tile::SandstoneSide],
    },
    BlockDef {
        block: Block::RedSandstone,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::RedSandstoneTop, Tile::RedSandstoneBottom, Tile::RedSandstoneSide],
    },
    BlockDef {
        block: Block::Terracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Terracotta, Tile::Terracotta, Tile::Terracotta],
    },
    BlockDef {
        block: Block::WhiteTerracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::WhiteTerracotta, Tile::WhiteTerracotta, Tile::WhiteTerracotta],
    },
    BlockDef {
        block: Block::OrangeTerracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::OrangeTerracotta, Tile::OrangeTerracotta, Tile::OrangeTerracotta],
    },
    BlockDef {
        block: Block::YellowTerracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::YellowTerracotta, Tile::YellowTerracotta, Tile::YellowTerracotta],
    },
    BlockDef {
        block: Block::BrownTerracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::BrownTerracotta, Tile::BrownTerracotta, Tile::BrownTerracotta],
    },
    BlockDef {
        block: Block::RedTerracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::RedTerracotta, Tile::RedTerracotta, Tile::RedTerracotta],
    },
    BlockDef {
        block: Block::LightGrayTerracotta,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::LightGrayTerracotta, Tile::LightGrayTerracotta, Tile::LightGrayTerracotta],
    },
    BlockDef {
        block: Block::Podzol,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::PodzolTop, Tile::Dirt, Tile::PodzolSide],
    },
    BlockDef {
        block: Block::Mycelium,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::MyceliumTop, Tile::Dirt, Tile::MyceliumSide],
    },
    BlockDef {
        block: Block::CoarseDirt,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::CoarseDirt, Tile::CoarseDirt, Tile::CoarseDirt],
    },
    BlockDef {
        block: Block::Gravel,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Gravel, Tile::Gravel, Tile::Gravel],
    },
    BlockDef {
        block: Block::Clay,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Clay, Tile::Clay, Tile::Clay],
    },
    BlockDef {
        block: Block::Mud,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Mud, Tile::Mud, Tile::Mud],
    },
    BlockDef {
        block: Block::MossBlock,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::MossBlock, Tile::MossBlock, Tile::MossBlock],
    },
    BlockDef {
        block: Block::SnowBlock,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Snow, Tile::Snow, Tile::Snow],
    },
    BlockDef {
        block: Block::PackedIce,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::PackedIce, Tile::PackedIce, Tile::PackedIce],
    },
    BlockDef {
        block: Block::Ice,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Ice, Tile::Ice, Tile::Ice],
    },
    BlockDef {
        block: Block::Calcite,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Calcite, Tile::Calcite, Tile::Calcite],
    },
    BlockDef {
        block: Block::Granite,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Granite, Tile::Granite, Tile::Granite],
    },
    BlockDef {
        block: Block::Diorite,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Diorite, Tile::Diorite, Tile::Diorite],
    },
    BlockDef {
        block: Block::Andesite,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Andesite, Tile::Andesite, Tile::Andesite],
    },
    BlockDef {
        block: Block::Tuff,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::Tuff, Tile::Tuff, Tile::Tuff],
    },
    BlockDef {
        block: Block::CoalOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::CoalOre, Tile::CoalOre, Tile::CoalOre],
    },
    BlockDef {
        block: Block::IronOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::IronOre, Tile::IronOre, Tile::IronOre],
    },
    BlockDef {
        block: Block::CopperOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::CopperOre, Tile::CopperOre, Tile::CopperOre],
    },
    BlockDef {
        block: Block::GoldOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::GoldOre, Tile::GoldOre, Tile::GoldOre],
    },
    BlockDef {
        block: Block::RedstoneOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::RedstoneOre, Tile::RedstoneOre, Tile::RedstoneOre],
    },
    BlockDef {
        block: Block::LapisOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::LapisOre, Tile::LapisOre, Tile::LapisOre],
    },
    BlockDef {
        block: Block::DiamondOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::DiamondOre, Tile::DiamondOre, Tile::DiamondOre],
    },
    BlockDef {
        block: Block::EmeraldOre,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::EmeraldOre, Tile::EmeraldOre, Tile::EmeraldOre],
    },
    BlockDef {
        block: Block::Pumpkin,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::PumpkinTop, Tile::PumpkinTop, Tile::PumpkinSide],
    },
    BlockDef {
        block: Block::Melon,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::MelonTop, Tile::MelonTop, Tile::MelonSide],
    },
    BlockDef {
        block: Block::Cactus,
        flags: FULL_CUBE_FLAGS,
        tiles: [Tile::CactusTop, Tile::CactusBottom, Tile::CactusSide],
    },
    BlockDef {
        block: Block::ShortGrass,
        flags: PLANT_FLAGS,
        tiles: [Tile::ShortGrass, Tile::ShortGrass, Tile::ShortGrass],
    },
    BlockDef {
        block: Block::Fern,
        flags: PLANT_FLAGS,
        tiles: [Tile::Fern, Tile::Fern, Tile::Fern],
    },
    BlockDef {
        block: Block::Dandelion,
        flags: PLANT_FLAGS,
        tiles: [Tile::Dandelion, Tile::Dandelion, Tile::Dandelion],
    },
    BlockDef {
        block: Block::Poppy,
        flags: PLANT_FLAGS,
        tiles: [Tile::Poppy, Tile::Poppy, Tile::Poppy],
    },
    BlockDef {
        block: Block::Cornflower,
        flags: PLANT_FLAGS,
        tiles: [Tile::Cornflower, Tile::Cornflower, Tile::Cornflower],
    },
    BlockDef {
        block: Block::Allium,
        flags: PLANT_FLAGS,
        tiles: [Tile::Allium, Tile::Allium, Tile::Allium],
    },
    BlockDef {
        block: Block::AzureBluet,
        flags: PLANT_FLAGS,
        tiles: [Tile::AzureBluet, Tile::AzureBluet, Tile::AzureBluet],
    },
    BlockDef {
        block: Block::OxeyeDaisy,
        flags: PLANT_FLAGS,
        tiles: [Tile::OxeyeDaisy, Tile::OxeyeDaisy, Tile::OxeyeDaisy],
    },
    BlockDef {
        block: Block::RedTulip,
        flags: PLANT_FLAGS,
        tiles: [Tile::RedTulip, Tile::RedTulip, Tile::RedTulip],
    },
    BlockDef {
        block: Block::DeadBush,
        flags: PLANT_FLAGS,
        tiles: [Tile::DeadBush, Tile::DeadBush, Tile::DeadBush],
    },
    BlockDef {
        block: Block::BrownMushroom,
        flags: PLANT_FLAGS,
        tiles: [Tile::BrownMushroom, Tile::BrownMushroom, Tile::BrownMushroom],
    },
    BlockDef {
        block: Block::RedMushroom,
        flags: PLANT_FLAGS,
        tiles: [Tile::RedMushroom, Tile::RedMushroom, Tile::RedMushroom],
    },
];

#[inline]
pub(super) fn from_id(id: u8) -> Block {
    BLOCK_DEFS
        .get(id as usize)
        .map_or(Block::Air, |def| def.block)
}

#[inline]
pub(super) fn def(block: Block) -> &'static BlockDef {
    let index = block.id() as usize;
    debug_assert!(
        index < BLOCK_DEFS.len() && BLOCK_DEFS[index].block == block,
        "BLOCK_DEFS must be ordered by Block::id()"
    );
    &BLOCK_DEFS[index]
}
