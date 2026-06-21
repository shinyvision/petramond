//! Item model: the inventory-space counterpart of `Block`.
//!
//! Every non-`Air` block has a matching `ItemType` variant in the SAME order and
//! with the SAME name, so item ids mirror block ids exactly (both start at
//! `Air = 0`). This 1:1 parity lets `ItemType::from_block` / `as_block` be plain
//! id conversions and keeps the two enums append-only in lock-step.
//!
//! Per-item static data (`name`, `max_stack_size`) lives in an id-ordered table
//! (`data::ITEM_DEFS`), mirroring `block/data.rs`. Behaviour that is derivable
//! from the underlying `Block` (`render_kind`) is computed via `Block`, not
//! stored, so adding a block-item never needs a second source of truth.

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};

mod data;
mod definition;

/// One variant per non-`Air` block-item, in the SAME order and with the SAME
/// names as [`Block`] (which also starts at `Air = 0`). Append-only: never
/// reorder or remove variants — ids are persisted-adjacent to block ids and are
/// covered by [`tests::ids_are_stable_and_append_only`].
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemType {
    Air = 0,
    Grass,
    Dirt,
    Stone,
    Sand,
    Snow,
    Water,
    OakLog,
    OakLeaves,
    SpruceLog,
    BirchLog,
    JungleLog,
    AcaciaLog,
    DarkOakLog,
    CherryLog,
    MangroveLog,
    SpruceLeaves,
    BirchLeaves,
    JungleLeaves,
    AcaciaLeaves,
    DarkOakLeaves,
    MangroveLeaves,
    CherryLeaves,
    AzaleaLeaves,
    RedSand,
    Sandstone,
    RedSandstone,
    Terracotta,
    WhiteTerracotta,
    OrangeTerracotta,
    YellowTerracotta,
    BrownTerracotta,
    RedTerracotta,
    LightGrayTerracotta,
    Podzol,
    Mycelium,
    CoarseDirt,
    Gravel,
    Clay,
    Mud,
    MossBlock,
    SnowBlock,
    PackedIce,
    Ice,
    Calcite,
    Granite,
    Diorite,
    Andesite,
    Tuff,
    CoalOre,
    IronOre,
    CopperOre,
    GoldOre,
    RedstoneOre,
    LapisOre,
    DiamondOre,
    EmeraldOre,
    Pumpkin,
    Melon,
    Cactus,
    ShortGrass,
    Fern,
    Dandelion,
    Poppy,
    Cornflower,
    Allium,
    AzureBluet,
    OxeyeDaisy,
    RedTulip,
    DeadBush,
    BrownMushroom,
    RedMushroom,
}

/// What a block drops when harvested. `(item, chance)`; an empty slice = no drop.
///
/// Lives here (not in `block/`) so block defs can reference it without an
/// ownership tangle (block defs already depend on the item crate path).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DropSpec {
    pub drops: &'static [(ItemType, f32)],
}

impl DropSpec {
    /// No drop at all (e.g. air, water, short grass).
    pub const NONE: DropSpec = DropSpec { drops: &[] };
}

/// How an item is drawn in inventory slots and in-hand.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemRenderKind {
    /// A full-cube block-item: render isometric 3D in slots, held as a 3D cube.
    BlockCube(Block),
    /// A flat sprite (cross-plant blocks like flowers/grass, and future tools):
    /// render the tile flat in slots, held as a flat billboard item.
    Sprite(Tile),
}

impl ItemType {
    /// All item types in id order (mirrors [`Block::ALL`]).
    pub const ALL: &'static [ItemType] = data::ALL_ITEMS;

    /// Stable numeric id, identical to the matching block's id.
    #[inline]
    pub fn id(self) -> u8 {
        self as u8
    }

    /// Item for `id`, or `Air` if `id` is out of range.
    #[inline]
    pub fn from_id(id: u8) -> ItemType {
        data::from_id(id)
    }

    /// The block-item for a block (1:1; `Air -> Air`). Because item ids mirror
    /// block ids, this is just an id conversion.
    #[inline]
    pub fn from_block(b: Block) -> ItemType {
        Self::from_id(b.id())
    }

    /// The block this item places (every 0.1 item maps back to a block, so this
    /// is always `Some`; kept as `Option` for future non-block items).
    #[inline]
    pub fn as_block(self) -> Option<Block> {
        Some(Block::from_id(self.id()))
    }

    /// Maximum number of this item per stack (64 for all current block-items).
    #[inline]
    pub fn max_stack_size(self) -> u8 {
        self.def().max_stack_size
    }

    /// Human-readable display name.
    #[inline]
    pub fn name(self) -> &'static str {
        self.def().name
    }

    /// How to draw this item: `BlockCube` for full cubes, `Sprite(tile)` for
    /// cross-model plants. Derived from the underlying block's render shape so
    /// there is a single source of truth.
    #[inline]
    pub fn render_kind(self) -> ItemRenderKind {
        // Every 0.1 item maps to a block (incl. Air -> Air).
        let block = Block::from_id(self.id());
        match block.render_shape() {
            RenderShape::Cube => ItemRenderKind::BlockCube(block),
            RenderShape::Cross => ItemRenderKind::Sprite(block.tiles()[0]),
        }
    }

    #[inline]
    fn def(self) -> &'static definition::ItemDef {
        data::def(self)
    }
}

/// A run of identical items occupying one inventory slot.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemType,
    pub count: u8,
}

impl ItemStack {
    /// A stack of `count` `item`s, clamped to the item's max stack size.
    #[inline]
    pub fn new(item: ItemType, count: u8) -> Self {
        ItemStack {
            item,
            count: count.min(item.max_stack_size()),
        }
    }

    /// `true` if this slot holds nothing (`Air` or zero count).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.item == ItemType::Air || self.count == 0
    }

    /// `true` if `other` can merge into this stack (same non-empty item type).
    #[inline]
    pub fn can_stack_with(&self, other: &ItemStack) -> bool {
        self.item == other.item
    }

    /// How many more of this item fit before hitting the max stack size.
    #[inline]
    pub fn space_left(&self) -> u8 {
        self.item.max_stack_size().saturating_sub(self.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_append_only() {
        let expected = [
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

        assert_eq!(ItemType::ALL, expected);
        for (id, item) in expected.into_iter().enumerate() {
            assert_eq!(item.id(), id as u8);
            assert_eq!(ItemType::from_id(id as u8), item);
        }
        assert_eq!(ItemType::from_id(u8::MAX), ItemType::Air);
    }

    #[test]
    fn definitions_are_id_ordered() {
        assert_eq!(data::ITEM_DEFS.len(), ItemType::ALL.len());
        for def in data::ITEM_DEFS {
            assert_eq!(ItemType::from_id(def.item.id()), def.item);
            assert_eq!(data::ITEM_DEFS[def.item.id() as usize].item, def.item);
        }
    }

    #[test]
    fn item_block_id_parity() {
        // Every block maps to an item with the same id and back, and the two
        // enums have the same length (1:1 parity).
        assert_eq!(ItemType::ALL.len(), Block::ALL.len());
        for &block in Block::ALL {
            let item = ItemType::from_block(block);
            assert_eq!(item.id(), block.id(), "{block:?}");
            assert_eq!(item.as_block(), Some(block), "{block:?}");
        }
        // Air round-trips both ways.
        assert_eq!(ItemType::from_block(Block::Air), ItemType::Air);
        assert_eq!(ItemType::Air.as_block(), Some(Block::Air));
    }

    #[test]
    fn render_kind_matches_render_shape() {
        for &block in Block::ALL {
            let item = ItemType::from_block(block);
            match block.render_shape() {
                RenderShape::Cube => {
                    assert_eq!(
                        item.render_kind(),
                        ItemRenderKind::BlockCube(block),
                        "{block:?}"
                    );
                }
                RenderShape::Cross => {
                    assert_eq!(
                        item.render_kind(),
                        ItemRenderKind::Sprite(block.tiles()[0]),
                        "{block:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn stack_basics() {
        // new clamps to max stack size.
        let s = ItemStack::new(ItemType::Stone, 200);
        assert_eq!(s.count, 64);
        assert_eq!(s.space_left(), 0);

        let s = ItemStack::new(ItemType::Dirt, 10);
        assert!(!s.is_empty());
        assert_eq!(s.space_left(), 54);
        assert!(s.can_stack_with(&ItemStack::new(ItemType::Dirt, 1)));
        assert!(!s.can_stack_with(&ItemStack::new(ItemType::Stone, 1)));

        // Empty cases.
        assert!(ItemStack::new(ItemType::Air, 5).is_empty());
        assert!(ItemStack::new(ItemType::Dirt, 0).is_empty());
    }

    #[test]
    fn drop_spec_none_is_empty() {
        assert!(DropSpec::NONE.drops.is_empty());
    }
}
