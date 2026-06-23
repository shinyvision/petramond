//! Item model: the inventory-space counterpart of `Block`.
//!
//! Item ids mirror block ids for the block-items: every `Block` has an `ItemType`
//! of the SAME id (both start at `Air = 0`), so `from_block` / `as_block` are
//! plain id conversions over the block range. Beyond that range live the
//! item-only variants (tools, raw drops) that have NO block — `as_block` returns
//! `None` and they render as flat sprites. Both enums stay append-only.
//!
//! Per-item static data (`key`, `name`, `max_stack_size`) lives in an id-ordered
//! table (`data::ITEM_DEFS`), mirroring `block/data.rs`. The `key` is the stable
//! recipe identity; `name` is display-only. Behaviour derivable from the
//! underlying `Block` (`render_kind` for block-items) is computed via `Block`;
//! item-only sprites + pickaxe tiers are small matches, not table columns.

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
    // --- Crafting update: block-items, mirroring the new Block ids 70..79. ---
    Cobblestone,
    OakPlanks,
    SprucePlanks,
    BirchPlanks,
    JunglePlanks,
    AcaciaPlanks,
    DarkOakPlanks,
    CherryPlanks,
    MangrovePlanks,
    CraftingTable,
    // --- Item-only variants (no Block): tools + raw drops. `as_block()` = None. ---
    Stick,
    WoodenPickaxe,
    StonePickaxe,
    RawIron,
    RawCopper,
    Coal,
    // --- Furnace update: smelted ingots (item-only) and the Furnace block-item.
    // Appended at the END so every id above stays frozen. Unlike the earlier
    // block-items, `ItemType::Furnace` does NOT share `Block::Furnace`'s id — the
    // block↔item mapping is made explicit in `from_block` / `as_block` instead of
    // assuming id equality (see `LEGACY_BLOCK_ITEMS`). ---
    IronIngot,
    CopperIngot,
    Furnace,
    // --- Chest update: the Chest block-item. Like the furnace, its item id is
    // appended (NOT equal to `Block::Chest`'s id) and mapped explicitly below. ---
    Chest,
}

/// One harvested drop: `min..=max` of `item`. A range (e.g. copper's 2–4) is
/// rolled at spawn time; `min == max` is an exact count.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Drop {
    pub item: ItemType,
    pub min: u8,
    pub max: u8,
}

/// What a block drops when harvested (with a sufficient tool, per the mining
/// model). An empty slice = no drop.
///
/// Lives here (not in `block/`) so block defs can reference it without an
/// ownership tangle (block defs already depend on the item crate path).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DropSpec {
    pub drops: &'static [Drop],
}

impl DropSpec {
    /// No drop at all (e.g. air, water, short grass).
    pub const NONE: DropSpec = DropSpec { drops: &[] };
}

/// A named group of items shared across recipes (e.g. any wood planks). Tags are
/// a PROPERTY OF ITEMS: each item lists its tags in its [`ItemDef`](definition::ItemDef)
/// data row, a recipe references a tag by name, and the crafting matcher asks each
/// item whether it carries the tag (see [`ItemType::has_tag`]). Keeping membership
/// in item data (not the recipe loader) means a new item joins a group by editing
/// its data row, never any recipe code.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemTag {
    /// Any wood-type planks (mirrors Minecraft's `#planks`).
    Planks,
    /// Anything that burns as furnace fuel — shift-clicked into the fuel slot.
    Fuel,
    /// Anything a furnace can smelt — shift-clicked into the input slot.
    Smeltable,
}

impl ItemTag {
    /// Resolve a tag's registry name (the text after `#` in a recipe) to its tag,
    /// or `None` if unknown.
    pub fn from_key(key: &str) -> Option<ItemTag> {
        match key {
            "planks" => Some(ItemTag::Planks),
            "fuel" => Some(ItemTag::Fuel),
            "smeltable" => Some(ItemTag::Smeltable),
            _ => None,
        }
    }
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

/// First-person hold orientation for a [`Sprite`](ItemRenderKind::Sprite) item:
/// the Euler tilt (radians) applied to the upright, origin-centred extruded slab
/// before it's seated in the hand (see [`crate::render`]'s `held_sprite`). A long
/// tool is laid diagonally like a swung handle (`roll != 0`); a small item stands
/// upright (`roll == 0`). Per-item so each item can declare how it's held.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeldPose {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl HeldPose {
    /// Upright hold for an ordinary sprite item (flowers, raw drops): no roll, so
    /// it stands straight up in the hand. The shared default carried by every
    /// item that isn't a tool with its own pose.
    pub const DEFAULT: HeldPose = HeldPose {
        pitch: 0.0,
        yaw: 1.8,
        roll: 0.0,
    };
}

impl ItemType {
    /// All item types in id order (mirrors [`Block::ALL`]).
    pub const ALL: &'static [ItemType] = data::ALL_ITEMS;

    /// Size of the original 0.1 block-item prefix: item ids `[0, LEGACY_BLOCK_ITEMS)`
    /// are block-items that share their block's id (`Air..=CraftingTable`). Block-items
    /// added afterwards are appended past the item-only range and mapped explicitly in
    /// [`from_block`](Self::from_block)/[`as_block`](Self::as_block), so growing the
    /// block list never shifts an item id. Pinned by `item_block_id_parity`.
    const LEGACY_BLOCK_ITEMS: usize = Block::CraftingTable.id() as usize + 1;

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

    /// The block-item for a block, or `None` if the block has no inventory item.
    /// The original 0.1 block-items (`[0, LEGACY_BLOCK_ITEMS)`) share their block's
    /// id, so for them this is a plain id conversion; block-items added later are
    /// appended past the item-only range and mapped explicitly here, so adding a
    /// block never shifts an existing item id. `Air -> Air`.
    #[inline]
    pub fn from_block(b: Block) -> ItemType {
        match b {
            Block::Furnace => ItemType::Furnace,
            Block::Chest => ItemType::Chest,
            _ => Self::from_id(b.id()),
        }
    }

    /// The block this item places, or `None` for an item-only item (tools, raw
    /// drops, ingots). The frozen id-equal prefix `[0, LEGACY_BLOCK_ITEMS)` maps by
    /// id; later block-items (appended past the item-only items) are matched
    /// explicitly so their item id need not equal their block id.
    #[inline]
    pub fn as_block(self) -> Option<Block> {
        match self {
            ItemType::Furnace => Some(Block::Furnace),
            ItemType::Chest => Some(Block::Chest),
            _ if (self.id() as usize) < Self::LEGACY_BLOCK_ITEMS => {
                Some(Block::from_id(self.id()))
            }
            _ => None,
        }
    }

    /// Pickaxe mining tier: `0` = not a pickaxe (mines at the hand rate), `1` =
    /// wooden, `2` = stone. Drives tool-gated mining — see [`Block::harvest_tier`]
    /// and [`crate::mining::break_time`].
    #[inline]
    pub fn pickaxe_tier(self) -> u8 {
        match self {
            ItemType::WoodenPickaxe => 1,
            ItemType::StonePickaxe => 2,
            _ => 0,
        }
    }

    /// How many game ticks this item burns as furnace fuel (`0` = not a fuel).
    /// A property of the item — a furnace consuming it reads this, like mining
    /// reads [`pickaxe_tier`](Self::pickaxe_tier). One piece of coal burns 4800
    /// ticks (= eight 600-tick smelts).
    #[inline]
    pub fn fuel_burn_ticks(self) -> u16 {
        match self {
            ItemType::Coal => 4800,
            _ => 0,
        }
    }

    /// Whether this item belongs to `tag`. Membership is item data — each item's
    /// [`ItemDef`](definition::ItemDef) lists its tags — so recipes can require a
    /// group (e.g. any `#planks`) without naming every member, and a new item joins
    /// a group by editing its data row, never any recipe code.
    #[inline]
    pub fn has_tag(self, tag: ItemTag) -> bool {
        self.def().tags.contains(&tag)
    }

    /// Maximum number of this item per stack. Durable items never stack (one per
    /// slot); everything else uses its table value.
    #[inline]
    pub fn max_stack_size(self) -> u8 {
        if self.is_durable() {
            1
        } else {
            self.def().max_stack_size
        }
    }

    /// Whether this item carries durability. A durable item never stacks (one per
    /// slot) — that limit is a CONSEQUENCE of durability, not of being a "tool".
    /// Durability isn't consumed yet, but the model is correct: a future durable
    /// non-tool item would also not stack, for the same reason. The pickaxes are
    /// the only durable items so far.
    #[inline]
    pub fn is_durable(self) -> bool {
        matches!(self, ItemType::WoodenPickaxe | ItemType::StonePickaxe)
    }

    /// Stable snake_case identity recipes reference (e.g. `oak_planks`), read from
    /// the item's [`ItemDef`](definition::ItemDef) row. This is the item's real id,
    /// distinct from its [`name`](Self::name) display string — renaming the name
    /// never moves the key, so recipes keep resolving (see `crate::crafting::load`).
    #[inline]
    pub fn key(self) -> &'static str {
        self.def().key
    }

    /// Human-readable display name (UI only; the recipe identity is
    /// [`key`](Self::key)).
    #[inline]
    pub fn name(self) -> &'static str {
        self.def().name
    }

    /// How to draw this item. Block-items follow their block's render shape
    /// (`BlockCube` for full cubes, `Sprite` for cross-model plants); item-only
    /// items are always flat sprites pulled from [`item_sprite`](Self::item_sprite).
    #[inline]
    pub fn render_kind(self) -> ItemRenderKind {
        match self.as_block() {
            Some(block) => match block.render_shape() {
                RenderShape::Cube => ItemRenderKind::BlockCube(block),
                RenderShape::Cross => ItemRenderKind::Sprite(block.tiles()[0]),
            },
            None => ItemRenderKind::Sprite(self.item_sprite()),
        }
    }

    /// First-person hold orientation for this item when held as a sprite (tools,
    /// flowers, raw drops), read from its [`ItemDef`](definition::ItemDef) row.
    /// Pickaxes are laid diagonally like a swung tool; everything else carries
    /// [`HeldPose::DEFAULT`] (upright). Only meaningful for `Sprite` render-kind
    /// items — block-cube items use the cube hold transform instead.
    #[inline]
    pub fn held_pose(self) -> HeldPose {
        self.def().held_pose
    }

    /// The flat atlas sprite for an item-only item (tools + raw drops).
    /// Block-items get their icon from the underlying block and never call this.
    #[inline]
    fn item_sprite(self) -> Tile {
        use ItemType::*;
        match self {
            Stick => Tile::Stick,
            WoodenPickaxe => Tile::WoodenPickaxe,
            StonePickaxe => Tile::StonePickaxe,
            RawIron => Tile::RawIron,
            RawCopper => Tile::RawCopper,
            Coal => Tile::Coal,
            IronIngot => Tile::IronIngot,
            CopperIngot => Tile::CopperIngot,
            // Block-items (incl. Furnace) resolve via `as_block`; never reach here.
            _ => Tile::Stick,
        }
    }

    #[inline]
    fn def(self) -> &'static definition::ItemDef {
        data::def(self)
    }
}

/// One-line delegating call for the shared id-ordering test in [`crate::registry`]:
/// the `ITEM_DEFS` table is id-ordered and one-to-one with [`ItemType::ALL`].
#[cfg(test)]
pub(crate) fn assert_registry_ordered() {
    crate::registry::assert_id_ordered(data::ITEM_DEFS, ItemType::ALL);
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
            ItemType::Cobblestone,
            ItemType::OakPlanks,
            ItemType::SprucePlanks,
            ItemType::BirchPlanks,
            ItemType::JunglePlanks,
            ItemType::AcaciaPlanks,
            ItemType::DarkOakPlanks,
            ItemType::CherryPlanks,
            ItemType::MangrovePlanks,
            ItemType::CraftingTable,
            ItemType::Stick,
            ItemType::WoodenPickaxe,
            ItemType::StonePickaxe,
            ItemType::RawIron,
            ItemType::RawCopper,
            ItemType::Coal,
            ItemType::IronIngot,
            ItemType::CopperIngot,
            ItemType::Furnace,
            ItemType::Chest,
        ];

        assert_eq!(ItemType::ALL, expected);
        for (id, item) in expected.into_iter().enumerate() {
            assert_eq!(item.id(), id as u8);
            assert_eq!(ItemType::from_id(id as u8), item);
        }
        assert_eq!(ItemType::from_id(u8::MAX), ItemType::Air);
    }

    #[test]
    fn item_block_id_parity() {
        // Every block round-trips through its block-item — whether or not the item
        // id equals the block id.
        for &block in Block::ALL {
            assert_eq!(
                ItemType::from_block(block).as_block(),
                Some(block),
                "{block:?} should round-trip block -> item -> block"
            );
        }
        // The frozen 0.1 prefix is still id-equal; the furnace block-item is
        // deliberately decoupled (its item id is appended, not the block id) — that
        // split is the point, so growing the block list never shifts an item id.
        for &block in Block::ALL {
            if (block.id() as usize) < ItemType::LEGACY_BLOCK_ITEMS {
                assert_eq!(ItemType::from_block(block).id(), block.id(), "{block:?}");
            }
        }
        assert_ne!(
            ItemType::Furnace.id(),
            Block::Furnace.id(),
            "furnace item id is decoupled from its block id"
        );
        // Item-only items (tools, raw drops, ingots) place no block.
        for item in [
            ItemType::Stick,
            ItemType::WoodenPickaxe,
            ItemType::StonePickaxe,
            ItemType::RawIron,
            ItemType::RawCopper,
            ItemType::Coal,
            ItemType::IronIngot,
            ItemType::CopperIngot,
        ] {
            assert_eq!(item.as_block(), None, "{item:?} should be item-only");
        }
        // Air round-trips both ways.
        assert_eq!(ItemType::from_block(Block::Air), ItemType::Air);
        assert_eq!(ItemType::Air.as_block(), Some(Block::Air));
    }

    #[test]
    fn item_only_items_render_as_sprites_and_carry_pickaxe_tiers() {
        for item in [
            ItemType::Stick,
            ItemType::WoodenPickaxe,
            ItemType::StonePickaxe,
            ItemType::RawIron,
            ItemType::RawCopper,
            ItemType::Coal,
        ] {
            assert_eq!(item.as_block(), None, "{item:?}");
            assert!(
                matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                "{item:?} should render as a sprite"
            );
        }
        // Pickaxe tiers gate tool mining; everything else is hand tier 0.
        assert_eq!(ItemType::WoodenPickaxe.pickaxe_tier(), 1);
        assert_eq!(ItemType::StonePickaxe.pickaxe_tier(), 2);
        assert_eq!(ItemType::Stick.pickaxe_tier(), 0);
        assert_eq!(ItemType::Cobblestone.pickaxe_tier(), 0);
    }

    #[test]
    fn held_pose_is_diagonal_for_pickaxes_upright_otherwise() {
        // Pickaxes hang diagonally in the hand (rolled like a swung handle)...
        for pick in [ItemType::WoodenPickaxe, ItemType::StonePickaxe] {
            assert_ne!(
                pick.held_pose().roll,
                0.0,
                "{pick:?} should hang diagonally"
            );
        }
        // ...every other sprite-held item stands upright (no roll).
        for upright in [
            ItemType::Poppy,
            ItemType::Stick,
            ItemType::RawIron,
            ItemType::Dandelion,
        ] {
            assert_eq!(
                upright.held_pose().roll,
                0.0,
                "{upright:?} should stand upright"
            );
        }
    }

    #[test]
    fn durable_items_do_not_stack() {
        // The stack limit of 1 follows from durability, not from being a "tool".
        for durable in [ItemType::WoodenPickaxe, ItemType::StonePickaxe] {
            assert!(durable.is_durable(), "{durable:?}");
            assert_eq!(durable.max_stack_size(), 1, "{durable:?}");
            // ItemStack clamps to the durable limit.
            assert_eq!(ItemStack::new(durable, 5).count, 1);
        }
        // Non-durable items keep their table stack size (sticks, raw drops, blocks).
        for stackable in [ItemType::Stick, ItemType::RawIron, ItemType::Cobblestone] {
            assert!(!stackable.is_durable(), "{stackable:?}");
            assert_eq!(stackable.max_stack_size(), 64, "{stackable:?}");
        }
    }

    #[test]
    fn item_tags_are_item_data() {
        use ItemTag::Planks;
        for p in [
            ItemType::OakPlanks,
            ItemType::SprucePlanks,
            ItemType::MangrovePlanks,
        ] {
            assert!(p.has_tag(Planks), "{p:?}");
        }
        // Logs and sticks are not planks.
        assert!(!ItemType::OakLog.has_tag(Planks));
        assert!(!ItemType::Stick.has_tag(Planks));
        // Tag names resolve from the recipe key.
        assert_eq!(ItemTag::from_key("planks"), Some(Planks));
        assert_eq!(ItemTag::from_key("bogus"), None);

        // Furnace routing tags: coal is fuel; raw ores are smeltable; the products
        // are neither (so a finished ingot doesn't shift back into the furnace).
        assert!(ItemType::Coal.has_tag(ItemTag::Fuel));
        assert!(!ItemType::Coal.has_tag(ItemTag::Smeltable));
        assert!(ItemType::RawIron.has_tag(ItemTag::Smeltable));
        assert!(ItemType::RawCopper.has_tag(ItemTag::Smeltable));
        assert!(!ItemType::RawIron.has_tag(ItemTag::Fuel));
        assert!(!ItemType::IronIngot.has_tag(ItemTag::Smeltable));
        assert!(!ItemType::IronIngot.has_tag(ItemTag::Fuel));
        assert_eq!(ItemTag::from_key("fuel"), Some(ItemTag::Fuel));
        assert_eq!(ItemTag::from_key("smeltable"), Some(ItemTag::Smeltable));
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
