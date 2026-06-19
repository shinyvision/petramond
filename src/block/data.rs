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
