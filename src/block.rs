//! Block registry + per-face tile mapping.

use crate::atlas::Tile;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Air = 0,
    Grass,
    Dirt,
    Stone,
    Sand,
    Snow,
    Water,
    OakLog,
    OakLeaves,
}

impl Block {
    pub const ALL: &'static [Block] = &[
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

    pub fn id(self) -> u8 { self as u8 }

    pub fn from_id(id: u8) -> Block {
        match id {
            1 => Block::Grass,
            2 => Block::Dirt,
            3 => Block::Stone,
            4 => Block::Sand,
            5 => Block::Snow,
            6 => Block::Water,
            7 => Block::OakLog,
            8 => Block::OakLeaves,
            _ => Block::Air,
        }
    }

    pub fn is_solid(self) -> bool {
        !matches!(self, Block::Air | Block::Water)
    }

    pub fn is_opaque(self) -> bool {
        !matches!(self, Block::Air | Block::Water | Block::OakLeaves)
    }

    /// Does this block cast ambient occlusion? Full opaque cubes always do, and
    /// leaves also occlude — onto adjacent leaves and within a canopy — so dense
    /// foliage gets internal AO depth instead of reading flat. Unlike `is_opaque`,
    /// this does NOT affect face culling or skylight (leaves still draw every face
    /// and still pass light through at half attenuation). Water never occludes.
    pub fn occludes_ao(self) -> bool {
        self.is_opaque() || matches!(self, Block::OakLeaves)
    }

    pub fn is_transparent(self) -> bool {
        matches!(self, Block::Water | Block::OakLeaves)
    }

    /// Per-face tile: [top, bottom, side].
    pub fn tiles(self) -> [Tile; 3] {
        use Tile::*;
        match self {
            Block::Air => [OakLeaves, OakLeaves, OakLeaves], // unused
            Block::Grass => [GrassTop, Dirt, GrassSide],
            Block::Dirt => [Dirt, Dirt, Dirt],
            Block::Stone => [Stone, Stone, Stone],
            Block::Sand => [Sand, Sand, Sand],
            Block::Snow => [Snow, Dirt, GrassSnow],
            Block::Water => [Water, Water, Water],
            Block::OakLog => [OakLogTop, OakLogTop, OakLogSide],
            Block::OakLeaves => [OakLeaves, OakLeaves, OakLeaves],
        }
    }
}