//! Block registry + per-face tile mapping.

use crate::atlas::Tile;

mod data;
mod definition;

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
    pub const ALL: &'static [Block] = data::ALL_BLOCKS;

    #[inline]
    pub fn id(self) -> u8 {
        self as u8
    }

    #[inline]
    pub fn from_id(id: u8) -> Block {
        data::from_id(id)
    }

    #[inline]
    pub fn is_solid(self) -> bool {
        self.def().flags.is_solid()
    }

    #[inline]
    pub fn is_opaque(self) -> bool {
        self.def().flags.is_opaque()
    }

    /// Does this block cast ambient occlusion? Full opaque cubes always do, and
    /// leaves also occlude — onto adjacent leaves and within a canopy — so dense
    /// foliage gets internal AO depth instead of reading flat. Unlike `is_opaque`,
    /// this does NOT affect face culling or skylight (leaves still draw every face
    /// and still pass light through at half attenuation). Water never occludes.
    #[inline]
    pub fn occludes_ao(self) -> bool {
        self.def().flags.occludes_ao()
    }

    #[inline]
    pub fn is_transparent(self) -> bool {
        self.def().flags.is_transparent()
    }

    /// A cell a placement may overwrite: empty air, or water (building into water
    /// displaces it). Mirrors the place-gate in app::handle_block_actions.
    #[inline]
    pub fn is_replaceable(self) -> bool {
        self.def().flags.is_replaceable()
    }

    /// Per-face tile: [top, bottom, side].
    #[inline]
    pub fn tiles(self) -> [Tile; 3] {
        self.def().tiles
    }

    #[inline]
    fn def(self) -> &'static definition::BlockDef {
        data::def(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{data, Block};
    use crate::atlas::Tile;

    #[test]
    fn ids_are_stable_and_append_only() {
        let expected = [
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

        assert_eq!(Block::ALL, expected);
        for (id, block) in expected.into_iter().enumerate() {
            assert_eq!(block.id(), id as u8);
            assert_eq!(Block::from_id(id as u8), block);
        }
        assert_eq!(Block::from_id(u8::MAX), Block::Air);
    }

    #[test]
    fn definitions_are_id_ordered() {
        assert_eq!(data::BLOCK_DEFS.len(), Block::ALL.len());
        for def in data::BLOCK_DEFS {
            assert_eq!(Block::from_id(def.block.id()), def.block);
            assert_eq!(data::BLOCK_DEFS[def.block.id() as usize].block, def.block);
        }
    }

    #[test]
    fn properties_match_existing_behavior() {
        assert!(!Block::Air.is_solid());
        assert!(!Block::Air.is_opaque());
        assert!(!Block::Air.occludes_ao());
        assert!(!Block::Air.is_transparent());
        assert!(Block::Air.is_replaceable());

        for block in [
            Block::Grass,
            Block::Dirt,
            Block::Stone,
            Block::Sand,
            Block::Snow,
            Block::OakLog,
        ] {
            assert!(block.is_solid(), "{block:?}");
            assert!(block.is_opaque(), "{block:?}");
            assert!(block.occludes_ao(), "{block:?}");
            assert!(!block.is_transparent(), "{block:?}");
            assert!(!block.is_replaceable(), "{block:?}");
        }

        assert!(!Block::Water.is_solid());
        assert!(!Block::Water.is_opaque());
        assert!(!Block::Water.occludes_ao());
        assert!(Block::Water.is_transparent());
        assert!(Block::Water.is_replaceable());

        assert!(Block::OakLeaves.is_solid());
        assert!(!Block::OakLeaves.is_opaque());
        assert!(Block::OakLeaves.occludes_ao());
        assert!(Block::OakLeaves.is_transparent());
        assert!(!Block::OakLeaves.is_replaceable());
    }

    #[test]
    fn tiles_match_existing_face_mapping() {
        assert_eq!(
            Block::Air.tiles(),
            [Tile::OakLeaves, Tile::OakLeaves, Tile::OakLeaves]
        );
        assert_eq!(
            Block::Grass.tiles(),
            [Tile::GrassTop, Tile::Dirt, Tile::GrassSide]
        );
        assert_eq!(Block::Dirt.tiles(), [Tile::Dirt, Tile::Dirt, Tile::Dirt]);
        assert_eq!(
            Block::Stone.tiles(),
            [Tile::Stone, Tile::Stone, Tile::Stone]
        );
        assert_eq!(Block::Sand.tiles(), [Tile::Sand, Tile::Sand, Tile::Sand]);
        assert_eq!(
            Block::Snow.tiles(),
            [Tile::Snow, Tile::Dirt, Tile::GrassSnow]
        );
        assert_eq!(
            Block::Water.tiles(),
            [Tile::Water, Tile::Water, Tile::Water]
        );
        assert_eq!(
            Block::OakLog.tiles(),
            [Tile::OakLogTop, Tile::OakLogTop, Tile::OakLogSide]
        );
        assert_eq!(
            Block::OakLeaves.tiles(),
            [Tile::OakLeaves, Tile::OakLeaves, Tile::OakLeaves]
        );
    }
}
