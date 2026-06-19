use crate::atlas::Tile;

use super::Block;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct BlockDef {
    pub block: Block,
    pub flags: BlockFlags,
    /// Per-face tile: [top, bottom, side].
    pub tiles: [Tile; 3],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct BlockFlags(u8);

impl BlockFlags {
    pub const SOLID: BlockFlags = BlockFlags(1 << 0);
    pub const OPAQUE: BlockFlags = BlockFlags(1 << 1);
    pub const AO_OCCLUDER: BlockFlags = BlockFlags(1 << 2);
    pub const TRANSPARENT: BlockFlags = BlockFlags(1 << 3);
    pub const REPLACEABLE: BlockFlags = BlockFlags(1 << 4);

    #[inline]
    pub const fn with(self, flag: BlockFlags) -> BlockFlags {
        BlockFlags(self.0 | flag.0)
    }

    #[inline]
    pub const fn is_solid(self) -> bool {
        self.contains(BlockFlags::SOLID)
    }

    #[inline]
    pub const fn is_opaque(self) -> bool {
        self.contains(BlockFlags::OPAQUE)
    }

    #[inline]
    pub const fn occludes_ao(self) -> bool {
        self.contains(BlockFlags::AO_OCCLUDER)
    }

    #[inline]
    pub const fn is_transparent(self) -> bool {
        self.contains(BlockFlags::TRANSPARENT)
    }

    #[inline]
    pub const fn is_replaceable(self) -> bool {
        self.contains(BlockFlags::REPLACEABLE)
    }

    #[inline]
    const fn contains(self, flag: BlockFlags) -> bool {
        self.0 & flag.0 == flag.0
    }
}
