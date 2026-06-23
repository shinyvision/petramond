use crate::atlas::Tile;
use crate::item::DropSpec;

use super::behavior::BlockBehavior;
use super::{Block, BlockTag};

// No `Debug`/`PartialEq`: the `behavior` trait object is neither, and nothing
// compares or formats a whole `BlockDef` (callers read individual fields).
#[derive(Copy, Clone)]
pub(super) struct BlockDef {
    pub block: Block,
    pub flags: BlockFlags,
    /// Category memberships (see [`BlockTag`]) — what this block *is*. Most rows
    /// carry none (`&[]`); a member lists each tag it belongs to. Mirrors the
    /// item table's `tags`.
    pub tags: &'static [BlockTag],
    /// World-reactive behaviour (see [`BlockBehavior`]) — what this block *does*.
    /// Most rows are [`behavior::INERT`](super::behavior::INERT).
    pub behavior: &'static dyn BlockBehavior,
    /// Per-face tile: [top, bottom, side].
    pub tiles: [Tile; 3],
    /// Mining material class (drives tool requirement + future tool tiers).
    pub material: BlockMaterial,
    /// Minimum pickaxe tier to HARVEST this block (`0` = hand, `1` = wooden,
    /// `2` = stone, `3` = above stone). See [`Block::harvest_tier`](super::Block::harvest_tier).
    pub harvest_tier: u8,
    /// Base break time scalar in "hardness units"; `0.0` = instant, `< 0.0` =
    /// unbreakable (never a mining target). See `crate::mining` for the model.
    pub hardness: f32,
    /// What this block yields when harvested. `DropSpec::NONE` = no drop.
    pub drop: DropSpec,
}

/// Mining material class of a block — an internal mining-grouping key (drives the
/// tool requirement and groups blocks for tool tiers). Not part of the public
/// surface: callers use [`Block::requires_tool`](super::Block::requires_tool) /
/// [`Block::harvest_tier`](super::Block::harvest_tier) instead.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BlockMaterial {
    None,
    Dirt,
    Sand,
    Stone,
    Ore,
    Wood,
    Foliage,
    Plant,
    Other,
}

/// `drops self ×1` helper: a one-entry [`DropSpec`] yielding exactly one of the
/// block's own item. The slices are `'static` so they can live in [`BlockDef`].
macro_rules! drops_self {
    ($item:ident) => {
        DropSpec {
            drops: &[Drop {
                item: ItemType::$item,
                min: 1,
                max: 1,
            }],
        }
    };
}
pub(super) use drops_self;

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
