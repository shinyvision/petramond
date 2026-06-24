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
    // --- Torch update: the Torch block-item. Item id appended (NOT equal to
    // `Block::Torch`'s id) and mapped explicitly in `from_block` / `as_block`. ---
    Torch,
    // --- Tools + ores update (item-only, no Block): the new ore drops + smelted
    // gold, then the iron/diamond pickaxes and the four axe tiers. Appended at the
    // END so every id above stays frozen. `as_block()` = None for all of them. ---
    Diamond,
    LapisLazuli,
    RawGold,
    GoldIngot,
    WoodenAxe,
    StoneAxe,
    IronAxe,
    DiamondAxe,
    IronPickaxe,
    DiamondPickaxe,
    // --- Shovels (item-only, no Block): the four shovel tiers, the dirt/sand
    // counterpart to the pickaxe (stone) and axe (wood). Appended at the END so
    // every id above stays frozen. `as_block()` = None for all of them. ---
    WoodenShovel,
    StoneShovel,
    IronShovel,
    DiamondShovel,
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

/// What family of tool an item is, for mining. A tool speeds up the block class
/// it is *for* — a [`Pickaxe`](ToolKind::Pickaxe) mines stone & ore, an
/// [`Axe`](ToolKind::Axe) mines wood, a [`Shovel`](ToolKind::Shovel) mines dirt &
/// sand — and a wrong-kind tool (an axe on stone, a shovel on a log) mines no
/// faster than a bare hand and unlocks no drop. The block half of this pairing is
/// [`Block::preferred_tool`](crate::block::Block::preferred_tool).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
}

impl ToolKind {
    /// How effective this kind of tool is at mining its own block class, as a
    /// multiplier on the shared material-tier speed ladder (see
    /// [`crate::mining::break_time`]). A pickaxe and an axe are the baseline
    /// (`1.0`); a shovel is a clumsier digging implement, so it clears its dirt &
    /// sand at `0.5625` of the speed an equal-tier pickaxe gets on stone —
    /// uniformly slower at every tier, because the factor scales the whole ladder.
    /// Tuned low enough that even a diamond shovel (the ×8 tier) tops out at ×4.5,
    /// the dirt-clearing rate of an iron-tier tool. This is a property of the tool
    /// KIND (the real reason a shovel digs slower), separate from the material
    /// `tier` it shares with the other kinds.
    #[inline]
    pub fn mining_efficiency(self) -> f32 {
        match self {
            ToolKind::Pickaxe | ToolKind::Axe => 1.0,
            // 0.5625 = 9/16: scales the ×8 diamond tier down to ×4.5.
            ToolKind::Shovel => 0.5625,
        }
    }
}

/// A mining tool: its [`kind`](Self::kind) and material `tier` (`1` = wooden,
/// `2` = stone, `3` = iron, `4` = diamond). Read from an item via
/// [`ItemType::tool`]; the mining model (see [`crate::mining`]) keys both the
/// speed multiplier and the harvest gate off it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub kind: ToolKind,
    pub tier: u8,
}

impl Tool {
    /// The melee damage range `(min, max)` this tool rolls per hit. A weapon's damage
    /// is a property of the tool itself — its KIND and material TIER: axes hit hardest,
    /// shovels and pickaxes share a gentler curve, and every diamond tool one-shots a
    /// small mob. The attacker rolls a uniform value in this range each swing, so a
    /// tool's hits-to-kill against a given mob spans a small band rather than a fixed
    /// count (a flat integer-per-hit couldn't produce e.g. "3–4 hits" on 4 health).
    pub fn attack_damage(self) -> (f32, f32) {
        use ToolKind::*;
        // Diamond is uniformly lethal regardless of kind.
        if self.tier >= 4 {
            return (5.0, 7.0);
        }
        match (self.kind, self.tier) {
            (Axe, 1) => (1.5, 2.5),
            (Axe, 2) => (2.0, 3.0),
            (Axe, 3) => (4.0, 6.0),
            // Shovels and pickaxes share a curve (clumsier weapons than an axe).
            (_, 1) => (1.0, 1.5),
            (_, 2) => (1.0, 2.5),
            (_, 3) => (2.5, 4.5),
            // Tiers are 1..=4; anything else falls back to the fist baseline.
            _ => FIST_DAMAGE,
        }
    }
}

/// Bare-hand (fist) melee damage — the baseline when nothing, or a non-weapon item, is
/// held. Deterministic: exactly 1 per hit (so a fist always takes 4 hits on 4 health).
pub const FIST_DAMAGE: (f32, f32) = (1.0, 1.0);

/// The melee damage range `(min, max)` for attacking with `item` in hand: the tool's
/// range if it's a weapon, else the [`FIST_DAMAGE`] baseline (an empty hand and a
/// non-weapon item like a block both punch for 1).
pub fn attack_damage(item: Option<ItemType>) -> (f32, f32) {
    item.and_then(ItemType::tool)
        .map(Tool::attack_damage)
        .unwrap_or(FIST_DAMAGE)
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
            Block::Torch => ItemType::Torch,
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
            ItemType::Torch => Some(Block::Torch),
            _ if (self.id() as usize) < Self::LEGACY_BLOCK_ITEMS => {
                Some(Block::from_id(self.id()))
            }
            _ => None,
        }
    }

    /// This item as a mining [`Tool`] (kind + material tier), or `None` if it
    /// isn't a tool. Drives tool-gated mining — the held tool's kind must match a
    /// block's [`preferred_tool`](crate::block::Block::preferred_tool) to mine it
    /// faster, and a pickaxe's tier must meet a block's
    /// [`harvest_tier`](crate::block::Block::harvest_tier) to unlock its drop (see
    /// [`crate::mining::break_time`]). The axe/pickaxe/shovel families share the
    /// tier ladder `1..=4` (wooden, stone, iron, diamond).
    #[inline]
    pub fn tool(self) -> Option<Tool> {
        use ItemType::*;
        use ToolKind::*;
        let (kind, tier) = match self {
            WoodenPickaxe => (Pickaxe, 1),
            StonePickaxe => (Pickaxe, 2),
            IronPickaxe => (Pickaxe, 3),
            DiamondPickaxe => (Pickaxe, 4),
            WoodenAxe => (Axe, 1),
            StoneAxe => (Axe, 2),
            IronAxe => (Axe, 3),
            DiamondAxe => (Axe, 4),
            WoodenShovel => (Shovel, 1),
            StoneShovel => (Shovel, 2),
            IronShovel => (Shovel, 3),
            DiamondShovel => (Shovel, 4),
            _ => return None,
        };
        Some(Tool { kind, tier })
    }

    /// How many game ticks this item burns as furnace fuel (`0` = not a fuel).
    /// A property of the item — a furnace consuming it reads this, like mining
    /// reads [`tool`](Self::tool). One piece of coal burns 4800
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
    /// non-tool item would also not stack, for the same reason. Every mining
    /// [`tool`](Self::tool) (the pickaxes, axes + shovels) is durable; nothing else is.
    #[inline]
    pub fn is_durable(self) -> bool {
        self.tool().is_some()
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
                // A torch isn't a cube; it shows the full torch sprite as a flat
                // hotbar icon and an extruded sprite in-hand (like a flower), not
                // the cropped per-face tiles the in-world pole uses.
                RenderShape::Torch => ItemRenderKind::Sprite(Tile::Torch),
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
            IronPickaxe => Tile::IronPickaxe,
            DiamondPickaxe => Tile::DiamondPickaxe,
            WoodenAxe => Tile::WoodenAxe,
            StoneAxe => Tile::StoneAxe,
            IronAxe => Tile::IronAxe,
            DiamondAxe => Tile::DiamondAxe,
            WoodenShovel => Tile::WoodenShovel,
            StoneShovel => Tile::StoneShovel,
            IronShovel => Tile::IronShovel,
            DiamondShovel => Tile::DiamondShovel,
            RawIron => Tile::RawIron,
            RawCopper => Tile::RawCopper,
            RawGold => Tile::RawGold,
            Coal => Tile::Coal,
            IronIngot => Tile::IronIngot,
            CopperIngot => Tile::CopperIngot,
            GoldIngot => Tile::GoldIngot,
            Diamond => Tile::Diamond,
            LapisLazuli => Tile::LapisLazuli,
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
    fn attack_damage_ranges_are_ordered_and_positive() {
        // Mechanic, not the tuned numbers (which are free to change): an empty hand and
        // a non-weapon item both punch for exactly 1, and every item's range is a valid,
        // positive `lo <= hi`.
        assert_eq!(attack_damage(None), (1.0, 1.0), "fist is a deterministic 1");
        assert_eq!(attack_damage(Some(ItemType::Dirt)), (1.0, 1.0), "a non-weapon punches like a fist");
        for &it in ItemType::ALL {
            let (lo, hi) = attack_damage(Some(it));
            assert!(lo > 0.0 && lo <= hi, "{it:?}: invalid range {lo}..{hi}");
        }
        // Every diamond tool one-shots a 4-health mob (its minimum damage alone is lethal).
        for it in [ItemType::DiamondPickaxe, ItemType::DiamondAxe, ItemType::DiamondShovel] {
            assert!(attack_damage(Some(it)).0 >= 4.0, "a diamond tool one-shots: {it:?}");
        }
    }

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
            ItemType::Torch,
            ItemType::Diamond,
            ItemType::LapisLazuli,
            ItemType::RawGold,
            ItemType::GoldIngot,
            ItemType::WoodenAxe,
            ItemType::StoneAxe,
            ItemType::IronAxe,
            ItemType::DiamondAxe,
            ItemType::IronPickaxe,
            ItemType::DiamondPickaxe,
            ItemType::WoodenShovel,
            ItemType::StoneShovel,
            ItemType::IronShovel,
            ItemType::DiamondShovel,
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
    fn item_only_items_render_as_sprites_and_carry_tools() {
        for item in [
            ItemType::Stick,
            ItemType::WoodenPickaxe,
            ItemType::DiamondPickaxe,
            ItemType::IronAxe,
            ItemType::DiamondShovel,
            ItemType::RawIron,
            ItemType::RawGold,
            ItemType::Diamond,
            ItemType::LapisLazuli,
            ItemType::GoldIngot,
            ItemType::Coal,
        ] {
            assert_eq!(item.as_block(), None, "{item:?}");
            assert!(
                matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                "{item:?} should render as a sprite"
            );
        }
        // Tools carry a kind + tier (gating mining); non-tools carry none. The
        // three families share the 1..=4 tier ladder (wooden, stone, iron, diamond).
        use ToolKind::{Axe, Pickaxe, Shovel};
        assert_eq!(ItemType::WoodenPickaxe.tool(), Some(Tool { kind: Pickaxe, tier: 1 }));
        assert_eq!(ItemType::StonePickaxe.tool(), Some(Tool { kind: Pickaxe, tier: 2 }));
        assert_eq!(ItemType::IronPickaxe.tool(), Some(Tool { kind: Pickaxe, tier: 3 }));
        assert_eq!(ItemType::DiamondPickaxe.tool(), Some(Tool { kind: Pickaxe, tier: 4 }));
        assert_eq!(ItemType::WoodenAxe.tool(), Some(Tool { kind: Axe, tier: 1 }));
        assert_eq!(ItemType::DiamondAxe.tool(), Some(Tool { kind: Axe, tier: 4 }));
        assert_eq!(ItemType::WoodenShovel.tool(), Some(Tool { kind: Shovel, tier: 1 }));
        assert_eq!(ItemType::StoneShovel.tool(), Some(Tool { kind: Shovel, tier: 2 }));
        assert_eq!(ItemType::IronShovel.tool(), Some(Tool { kind: Shovel, tier: 3 }));
        assert_eq!(ItemType::DiamondShovel.tool(), Some(Tool { kind: Shovel, tier: 4 }));
        assert_eq!(ItemType::Stick.tool(), None);
        assert_eq!(ItemType::Cobblestone.tool(), None);
    }

    #[test]
    fn durable_items_do_not_stack() {
        // The stack limit of 1 follows from durability, not from being a "tool".
        // Every mining tool — pickaxes, axes and shovels, all four tiers — is durable.
        for durable in [
            ItemType::WoodenPickaxe,
            ItemType::StonePickaxe,
            ItemType::IronPickaxe,
            ItemType::DiamondPickaxe,
            ItemType::WoodenAxe,
            ItemType::StoneAxe,
            ItemType::IronAxe,
            ItemType::DiamondAxe,
            ItemType::WoodenShovel,
            ItemType::StoneShovel,
            ItemType::IronShovel,
            ItemType::DiamondShovel,
        ] {
            assert!(durable.is_durable(), "{durable:?}");
            assert_eq!(durable.max_stack_size(), 1, "{durable:?}");
            // ItemStack clamps to the durable limit.
            assert_eq!(ItemStack::new(durable, 5).count, 1);
        }
        // Non-durable items keep their table stack size (sticks, raw drops, gems,
        // ingots, blocks).
        for stackable in [
            ItemType::Stick,
            ItemType::RawIron,
            ItemType::RawGold,
            ItemType::Diamond,
            ItemType::LapisLazuli,
            ItemType::GoldIngot,
            ItemType::Cobblestone,
        ] {
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
                RenderShape::Torch => {
                    assert_eq!(
                        item.render_kind(),
                        ItemRenderKind::Sprite(Tile::Torch),
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
