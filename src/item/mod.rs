//! Item model: the inventory-space counterpart of `Block`.
//!
//! Item ids mirror block ids for the original block-items: every early `Block`
//! has an `ItemType` of the SAME id (both start at `Air = 0`), so `from_block` /
//! `as_block` are plain id conversions over that range. Beyond it live the
//! item-only items (tools, raw drops) that have NO block — `as_block` returns
//! `None` and they render as flat sprites. Engine ids stay append-only; pack
//! items register past them and link to a block through their row's `block`
//! field (see [`crate::registry`]).
//!
//! Per-item static data (`key`, `name`, `max_stack_size`) lives in an id-ordered
//! table loaded from `assets/items.json`, mirroring `block/data.rs`. The `key` is
//! the stable recipe identity; `name` is display-only. Behaviour derivable from the
//! underlying `Block` (`render_kind` for block-items) is computed via `Block`;
//! item-only sprites + pickaxe tiers are small matches, not table columns.

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};

mod data;
mod definition;
mod load;

pub(crate) use data::ENGINE_ITEM_NAMES;

/// An item's engine-implemented right-click use, referenced from its
/// `items.json` row by name (`"use": "bucket_fill"`). The string-keyed
/// registry of engine handlers: [`from_name`](Self::from_name) resolves a
/// row's key at load, and the tick-side dispatch (`game::item_use`,
/// `game::placement`) matches on the resolved handler — never on concrete
/// item ids — so packs can put an engine use on their own items.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemUse {
    /// Scoop a targeted water source into the held item (the empty bucket).
    BucketFill,
    /// Empty the held item into the clicked cell as water (the full bucket).
    BucketPour,
    /// Shear the targeted mob (runs at the earlier shear stage, before block
    /// interaction — see `game::placement`'s `tick_place`).
    Shear,
}

impl ItemUse {
    /// Resolve an `items.json` `use` key to an engine handler. There is no
    /// namespaced (`mod_id:key`) form: a mod reacts to its item's use through
    /// the `item_use_pre` event instead of declaring a handler.
    pub fn from_name(name: &str) -> Option<ItemUse> {
        Some(match name {
            "bucket_fill" => ItemUse::BucketFill,
            "bucket_pour" => ItemUse::BucketPour,
            "shear" => ItemUse::Shear,
            _ => return None,
        })
    }
}

/// A registered item, identified by its opaque runtime id. Engine items own
/// the low ids in a compiled, frozen order (the named consts below — save
/// palettes depend on those ids/names never moving); mod packs register
/// additional ids at load through namespaced `items.json` rows (see
/// [`crate::registry`]). Serde carries an item as its registered NAME string.
///
/// The engine block-item prefix mirrors [`Block`] ids (both start at
/// `Air = 0`), so `from_block` / `as_block` are plain id conversions over that
/// range; later engine block-items map explicitly, and DYNAMIC items link to
/// their block through their row's `block` field.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct ItemType(pub u8);

/// Engine item consts, named like the enum variants they replaced so every
/// existing `ItemType::Stick` expression and match pattern keeps compiling.
#[allow(non_upper_case_globals)]
impl ItemType {
    pub const Air: ItemType = ItemType(0);
    pub const Grass: ItemType = ItemType(1);
    pub const Dirt: ItemType = ItemType(2);
    pub const Stone: ItemType = ItemType(3);
    pub const Sand: ItemType = ItemType(4);
    pub const Snow: ItemType = ItemType(5);
    pub const Water: ItemType = ItemType(6);
    pub const OakLog: ItemType = ItemType(7);
    pub const OakLeaves: ItemType = ItemType(8);
    pub const SpruceLog: ItemType = ItemType(9);
    pub const BirchLog: ItemType = ItemType(10);
    pub const JungleLog: ItemType = ItemType(11);
    pub const AcaciaLog: ItemType = ItemType(12);
    pub const DarkOakLog: ItemType = ItemType(13);
    pub const CherryLog: ItemType = ItemType(14);
    pub const MangroveLog: ItemType = ItemType(15);
    pub const SpruceLeaves: ItemType = ItemType(16);
    pub const BirchLeaves: ItemType = ItemType(17);
    pub const JungleLeaves: ItemType = ItemType(18);
    pub const AcaciaLeaves: ItemType = ItemType(19);
    pub const DarkOakLeaves: ItemType = ItemType(20);
    pub const MangroveLeaves: ItemType = ItemType(21);
    pub const CherryLeaves: ItemType = ItemType(22);
    pub const AzaleaLeaves: ItemType = ItemType(23);
    pub const RedSand: ItemType = ItemType(24);
    pub const Sandstone: ItemType = ItemType(25);
    pub const RedSandstone: ItemType = ItemType(26);
    pub const Terracotta: ItemType = ItemType(27);
    pub const WhiteTerracotta: ItemType = ItemType(28);
    pub const OrangeTerracotta: ItemType = ItemType(29);
    pub const YellowTerracotta: ItemType = ItemType(30);
    pub const BrownTerracotta: ItemType = ItemType(31);
    pub const RedTerracotta: ItemType = ItemType(32);
    pub const LightGrayTerracotta: ItemType = ItemType(33);
    pub const Podzol: ItemType = ItemType(34);
    pub const Mycelium: ItemType = ItemType(35);
    pub const CoarseDirt: ItemType = ItemType(36);
    pub const Gravel: ItemType = ItemType(37);
    pub const Clay: ItemType = ItemType(38);
    pub const Mud: ItemType = ItemType(39);
    pub const MossBlock: ItemType = ItemType(40);
    pub const SnowBlock: ItemType = ItemType(41);
    pub const PackedIce: ItemType = ItemType(42);
    pub const Ice: ItemType = ItemType(43);
    pub const Calcite: ItemType = ItemType(44);
    pub const Granite: ItemType = ItemType(45);
    pub const Diorite: ItemType = ItemType(46);
    pub const Andesite: ItemType = ItemType(47);
    pub const Tuff: ItemType = ItemType(48);
    pub const CoalOre: ItemType = ItemType(49);
    pub const IronOre: ItemType = ItemType(50);
    pub const CopperOre: ItemType = ItemType(51);
    pub const GoldOre: ItemType = ItemType(52);
    pub const RedstoneOre: ItemType = ItemType(53);
    pub const LapisOre: ItemType = ItemType(54);
    pub const DiamondOre: ItemType = ItemType(55);
    pub const EmeraldOre: ItemType = ItemType(56);
    pub const Pumpkin: ItemType = ItemType(57);
    pub const Melon: ItemType = ItemType(58);
    pub const Cactus: ItemType = ItemType(59);
    pub const ShortGrass: ItemType = ItemType(60);
    pub const Fern: ItemType = ItemType(61);
    pub const Dandelion: ItemType = ItemType(62);
    pub const Poppy: ItemType = ItemType(63);
    pub const Cornflower: ItemType = ItemType(64);
    pub const Allium: ItemType = ItemType(65);
    pub const AzureBluet: ItemType = ItemType(66);
    pub const OxeyeDaisy: ItemType = ItemType(67);
    pub const RedTulip: ItemType = ItemType(68);
    pub const DeadBush: ItemType = ItemType(69);
    pub const BrownMushroom: ItemType = ItemType(70);
    pub const RedMushroom: ItemType = ItemType(71);
    pub const Cobblestone: ItemType = ItemType(72);
    pub const OakPlanks: ItemType = ItemType(73);
    pub const SprucePlanks: ItemType = ItemType(74);
    pub const BirchPlanks: ItemType = ItemType(75);
    pub const JunglePlanks: ItemType = ItemType(76);
    pub const AcaciaPlanks: ItemType = ItemType(77);
    pub const DarkOakPlanks: ItemType = ItemType(78);
    pub const CherryPlanks: ItemType = ItemType(79);
    pub const MangrovePlanks: ItemType = ItemType(80);
    pub const CraftingTable: ItemType = ItemType(81);
    pub const Stick: ItemType = ItemType(82);
    pub const WoodenPickaxe: ItemType = ItemType(83);
    pub const StonePickaxe: ItemType = ItemType(84);
    pub const RawIron: ItemType = ItemType(85);
    pub const RawCopper: ItemType = ItemType(86);
    pub const Coal: ItemType = ItemType(87);
    pub const IronIngot: ItemType = ItemType(88);
    pub const CopperIngot: ItemType = ItemType(89);
    pub const Furnace: ItemType = ItemType(90);
    pub const Chest: ItemType = ItemType(91);
    pub const Torch: ItemType = ItemType(92);
    pub const Diamond: ItemType = ItemType(93);
    pub const LapisLazuli: ItemType = ItemType(94);
    pub const RawGold: ItemType = ItemType(95);
    pub const GoldIngot: ItemType = ItemType(96);
    pub const WoodenAxe: ItemType = ItemType(97);
    pub const StoneAxe: ItemType = ItemType(98);
    pub const IronAxe: ItemType = ItemType(99);
    pub const DiamondAxe: ItemType = ItemType(100);
    pub const IronPickaxe: ItemType = ItemType(101);
    pub const DiamondPickaxe: ItemType = ItemType(102);
    pub const WoodenShovel: ItemType = ItemType(103);
    pub const StoneShovel: ItemType = ItemType(104);
    pub const IronShovel: ItemType = ItemType(105);
    pub const DiamondShovel: ItemType = ItemType(106);
    pub const FurnitureWorkbench: ItemType = ItemType(107);
    pub const OakSapling: ItemType = ItemType(108);
    pub const SpruceSapling: ItemType = ItemType(109);
    pub const BirchSapling: ItemType = ItemType(110);
    pub const JungleSapling: ItemType = ItemType(111);
    pub const AcaciaSapling: ItemType = ItemType(112);
    pub const DarkOakSapling: ItemType = ItemType(113);
    pub const CherrySapling: ItemType = ItemType(114);
    pub const OakDoor: ItemType = ItemType(115);
    pub const SpruceDoor: ItemType = ItemType(116);
    pub const BirchDoor: ItemType = ItemType(117);
    pub const JungleDoor: ItemType = ItemType(118);
    pub const AcaciaDoor: ItemType = ItemType(119);
    pub const DarkOakDoor: ItemType = ItemType(120);
    pub const CherryDoor: ItemType = ItemType(121);
    pub const MangroveDoor: ItemType = ItemType(122);
    pub const RedwoodLog: ItemType = ItemType(123);
    pub const RedwoodLeaves: ItemType = ItemType(124);
    pub const RedwoodPlanks: ItemType = ItemType(125);
    pub const RedwoodDoor: ItemType = ItemType(126);
    pub const OakStairs: ItemType = ItemType(127);
    pub const SpruceStairs: ItemType = ItemType(128);
    pub const BirchStairs: ItemType = ItemType(129);
    pub const JungleStairs: ItemType = ItemType(130);
    pub const AcaciaStairs: ItemType = ItemType(131);
    pub const DarkOakStairs: ItemType = ItemType(132);
    pub const CherryStairs: ItemType = ItemType(133);
    pub const MangroveStairs: ItemType = ItemType(134);
    pub const RedwoodStairs: ItemType = ItemType(135);
    pub const CobblestoneStairs: ItemType = ItemType(136);
    pub const StoneStairs: ItemType = ItemType(137);
    pub const DirtStairs: ItemType = ItemType(138);
    pub const WoodenBucket: ItemType = ItemType(139);
    pub const WaterBucket: ItemType = ItemType(140);
    pub const Shears: ItemType = ItemType(141);
    pub const Wool: ItemType = ItemType(142);
    pub const BedFrame: ItemType = ItemType(143);
    pub const Bed: ItemType = ItemType(144);
    pub const OakSlab: ItemType = ItemType(145);
    pub const SpruceSlab: ItemType = ItemType(146);
    pub const BirchSlab: ItemType = ItemType(147);
    pub const JungleSlab: ItemType = ItemType(148);
    pub const AcaciaSlab: ItemType = ItemType(149);
    pub const DarkOakSlab: ItemType = ItemType(150);
    pub const CherrySlab: ItemType = ItemType(151);
    pub const MangroveSlab: ItemType = ItemType(152);
    pub const RedwoodSlab: ItemType = ItemType(153);
    pub const CobblestoneSlab: ItemType = ItemType(154);
    pub const StoneSlab: ItemType = ItemType(155);
    pub const DirtSlab: ItemType = ItemType(156);
    pub const Glass: ItemType = ItemType(157);
    pub const GlassPane: ItemType = ItemType(158);
}

impl std::fmt::Debug for ItemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_ITEM_NAMES.get(self.0 as usize) {
            Some(name) => write!(f, "ItemType({name})"),
            None => write!(f, "ItemType(#{})", self.0),
        }
    }
}

impl serde::Serialize for ItemType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match crate::registry::names().items.name(self.0) {
            Some(name) => s.serialize_str(name),
            None => Err(serde::ser::Error::custom(format!(
                "item id {} is not registered",
                self.0
            ))),
        }
    }
}

impl<'de> serde::Deserialize<'de> for ItemType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = std::borrow::Cow::<str>::deserialize(d)?;
        crate::registry::names()
            .items
            .id(&name)
            .map(ItemType)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown item '{name}'")))
    }
}

/// One harvested drop: `min..=max` of `item`, dropped with probability `chance`.
/// A range (e.g. copper's 2–4) is rolled at spawn time; `min == max` is an exact
/// count. `chance` is the independent probability this drop appears at all (`1.0`
/// = always, e.g. ore yields); a sub-1 chance models an occasional yield such as
/// the 10% sapling a broken or decayed leaf sheds.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Drop {
    pub item: ItemType,
    pub min: u8,
    pub max: u8,
    pub chance: f32,
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
///
/// The vocabulary is OPEN: engine tags are the named consts below (bare
/// snake_case in `items.json`, `#llama:<name>` in recipes); a pack introduces
/// its own tag by listing a namespaced `mod_id:name` on item rows and
/// referencing `#mod_id:name` in recipes (interned at load — see
/// [`crate::registry::TagTable`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ItemTag(u8);

/// Engine item-tag names, id-ordered to match the consts on [`ItemTag`].
static ITEM_TAGS: crate::registry::TagTable =
    crate::registry::TagTable::new(&["planks", "logs", "fuel", "smeltable"]);

impl ItemTag {
    /// Any wood-type planks (recipe key `#llama:planks`).
    pub const PLANKS: ItemTag = ItemTag(0);
    /// Any wood-type log (recipe key `#llama:logs`).
    pub const LOGS: ItemTag = ItemTag(1);
    /// Anything that burns as furnace fuel — shift-clicked into the fuel slot.
    pub const FUEL: ItemTag = ItemTag(2);
    /// Anything a furnace can smelt — shift-clicked into the input slot.
    pub const SMELTABLE: ItemTag = ItemTag(3);

    /// Resolve a tag's registry name (the text after `#` in a recipe, or an
    /// `items.json` row entry), interning an unseen namespaced pack tag.
    /// `None` only for invalid names (a bare non-engine name).
    pub fn from_key(key: &str) -> Option<ItemTag> {
        ITEM_TAGS.resolve(key).ok().map(ItemTag)
    }

    /// Loader-side [`from_key`](Self::from_key) that surfaces the error text.
    pub(crate) fn resolve(name: &str) -> Result<ItemTag, String> {
        ITEM_TAGS.resolve(name).map(ItemTag)
    }
}

/// What family of tool an item is, for mining. A tool speeds up the block class
/// it is *for* — a [`Pickaxe`](ToolKind::Pickaxe) mines stone & ore, an
/// [`Axe`](ToolKind::Axe) mines wood, a [`Shovel`](ToolKind::Shovel) mines dirt &
/// sand — and a wrong-kind tool (an axe on stone, a shovel on a log) mines no
/// faster than a bare hand and unlocks no drop. The block half of this pairing is
/// [`Block::preferred_tool`](crate::block::Block::preferred_tool).
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// A data-driven bbmodel block: render its actual baked model (cubes + the model
    /// atlas) in slots / in-hand / dropped, not a stand-in cube. See `crate::block_model`.
    Model(crate::block_model::BlockModelKind),
}

/// First-person hold orientation for a [`Sprite`](ItemRenderKind::Sprite) item:
/// the Euler tilt (radians) applied to the upright, origin-centred extruded slab
/// before it's seated in the hand (see [`crate::render`]'s `held_sprite`). A long
/// tool is laid diagonally like a swung handle (`roll != 0`); a small item stands
/// upright (`roll == 0`). Per-item so each item can declare how it's held.
#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// Every registered item in id order — engine first (frozen ids), then
    /// pack-registered items in load order (mirrors [`Block::all`]).
    pub fn all() -> &'static [ItemType] {
        data::all()
    }

    /// Size of the original 0.1 block-item prefix: item ids `[0, LEGACY_BLOCK_ITEMS)`
    /// are block-items that share their block's id (`Air..=CraftingTable`). Block-items
    /// added afterwards are appended past the item-only range and mapped explicitly in
    /// [`from_block`](Self::from_block)/[`as_block`](Self::as_block).
    const LEGACY_BLOCK_ITEMS: usize = Block::CraftingTable.id() as usize + 1;

    /// Stable numeric id.
    #[inline]
    pub const fn id(self) -> u8 {
        self.0
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
            Block::FurnitureWorkbench => ItemType::FurnitureWorkbench,
            Block::OakSapling => ItemType::OakSapling,
            Block::SpruceSapling => ItemType::SpruceSapling,
            Block::BirchSapling => ItemType::BirchSapling,
            Block::JungleSapling => ItemType::JungleSapling,
            Block::AcaciaSapling => ItemType::AcaciaSapling,
            Block::DarkOakSapling => ItemType::DarkOakSapling,
            Block::CherrySapling => ItemType::CherrySapling,
            Block::OakDoor => ItemType::OakDoor,
            Block::SpruceDoor => ItemType::SpruceDoor,
            Block::BirchDoor => ItemType::BirchDoor,
            Block::JungleDoor => ItemType::JungleDoor,
            Block::AcaciaDoor => ItemType::AcaciaDoor,
            Block::DarkOakDoor => ItemType::DarkOakDoor,
            Block::CherryDoor => ItemType::CherryDoor,
            Block::MangroveDoor => ItemType::MangroveDoor,
            Block::RedwoodLog => ItemType::RedwoodLog,
            Block::RedwoodLeaves => ItemType::RedwoodLeaves,
            Block::RedwoodPlanks => ItemType::RedwoodPlanks,
            Block::RedwoodDoor => ItemType::RedwoodDoor,
            Block::OakStairs => ItemType::OakStairs,
            Block::SpruceStairs => ItemType::SpruceStairs,
            Block::BirchStairs => ItemType::BirchStairs,
            Block::JungleStairs => ItemType::JungleStairs,
            Block::AcaciaStairs => ItemType::AcaciaStairs,
            Block::DarkOakStairs => ItemType::DarkOakStairs,
            Block::CherryStairs => ItemType::CherryStairs,
            Block::MangroveStairs => ItemType::MangroveStairs,
            Block::RedwoodStairs => ItemType::RedwoodStairs,
            Block::CobblestoneStairs => ItemType::CobblestoneStairs,
            Block::StoneStairs => ItemType::StoneStairs,
            Block::DirtStairs => ItemType::DirtStairs,
            Block::BedFrame => ItemType::BedFrame,
            Block::Bed => ItemType::Bed,
            Block::OakSlab => ItemType::OakSlab,
            Block::SpruceSlab => ItemType::SpruceSlab,
            Block::BirchSlab => ItemType::BirchSlab,
            Block::JungleSlab => ItemType::JungleSlab,
            Block::AcaciaSlab => ItemType::AcaciaSlab,
            Block::DarkOakSlab => ItemType::DarkOakSlab,
            Block::CherrySlab => ItemType::CherrySlab,
            Block::MangroveSlab => ItemType::MangroveSlab,
            Block::RedwoodSlab => ItemType::RedwoodSlab,
            Block::CobblestoneSlab => ItemType::CobblestoneSlab,
            Block::StoneSlab => ItemType::StoneSlab,
            Block::DirtSlab => ItemType::DirtSlab,
            Block::Glass => ItemType::Glass,
            Block::GlassPane => ItemType::GlassPane,
            _ if (b.id() as usize) < Self::LEGACY_BLOCK_ITEMS => Self::from_id(b.id()),
            // A pack-registered block: its item declares the link via its
            // row's `block` field. No linked item -> Air (nothing to hold).
            _ => data::item_for_block(b).unwrap_or(ItemType::Air),
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
            ItemType::FurnitureWorkbench => Some(Block::FurnitureWorkbench),
            ItemType::OakSapling => Some(Block::OakSapling),
            ItemType::SpruceSapling => Some(Block::SpruceSapling),
            ItemType::BirchSapling => Some(Block::BirchSapling),
            ItemType::JungleSapling => Some(Block::JungleSapling),
            ItemType::AcaciaSapling => Some(Block::AcaciaSapling),
            ItemType::DarkOakSapling => Some(Block::DarkOakSapling),
            ItemType::CherrySapling => Some(Block::CherrySapling),
            ItemType::OakDoor => Some(Block::OakDoor),
            ItemType::SpruceDoor => Some(Block::SpruceDoor),
            ItemType::BirchDoor => Some(Block::BirchDoor),
            ItemType::JungleDoor => Some(Block::JungleDoor),
            ItemType::AcaciaDoor => Some(Block::AcaciaDoor),
            ItemType::DarkOakDoor => Some(Block::DarkOakDoor),
            ItemType::CherryDoor => Some(Block::CherryDoor),
            ItemType::MangroveDoor => Some(Block::MangroveDoor),
            ItemType::RedwoodLog => Some(Block::RedwoodLog),
            ItemType::RedwoodLeaves => Some(Block::RedwoodLeaves),
            ItemType::RedwoodPlanks => Some(Block::RedwoodPlanks),
            ItemType::RedwoodDoor => Some(Block::RedwoodDoor),
            ItemType::OakStairs => Some(Block::OakStairs),
            ItemType::SpruceStairs => Some(Block::SpruceStairs),
            ItemType::BirchStairs => Some(Block::BirchStairs),
            ItemType::JungleStairs => Some(Block::JungleStairs),
            ItemType::AcaciaStairs => Some(Block::AcaciaStairs),
            ItemType::DarkOakStairs => Some(Block::DarkOakStairs),
            ItemType::CherryStairs => Some(Block::CherryStairs),
            ItemType::MangroveStairs => Some(Block::MangroveStairs),
            ItemType::RedwoodStairs => Some(Block::RedwoodStairs),
            ItemType::CobblestoneStairs => Some(Block::CobblestoneStairs),
            ItemType::StoneStairs => Some(Block::StoneStairs),
            ItemType::DirtStairs => Some(Block::DirtStairs),
            ItemType::BedFrame => Some(Block::BedFrame),
            ItemType::Bed => Some(Block::Bed),
            ItemType::OakSlab => Some(Block::OakSlab),
            ItemType::SpruceSlab => Some(Block::SpruceSlab),
            ItemType::BirchSlab => Some(Block::BirchSlab),
            ItemType::JungleSlab => Some(Block::JungleSlab),
            ItemType::AcaciaSlab => Some(Block::AcaciaSlab),
            ItemType::DarkOakSlab => Some(Block::DarkOakSlab),
            ItemType::CherrySlab => Some(Block::CherrySlab),
            ItemType::MangroveSlab => Some(Block::MangroveSlab),
            ItemType::RedwoodSlab => Some(Block::RedwoodSlab),
            ItemType::CobblestoneSlab => Some(Block::CobblestoneSlab),
            ItemType::StoneSlab => Some(Block::StoneSlab),
            ItemType::DirtSlab => Some(Block::DirtSlab),
            ItemType::Glass => Some(Block::Glass),
            ItemType::GlassPane => Some(Block::GlassPane),
            _ if (self.id() as usize) < Self::LEGACY_BLOCK_ITEMS => Some(Block::from_id(self.id())),
            // Engine item-only items carry no link; a pack item's row may
            // (`"block": "mod:key"` in items.json).
            _ => self.def().block,
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
        self.def().tool
    }

    /// How many game ticks this item burns as furnace fuel (`0` = not a fuel).
    /// A property of the item (`"fuel_burn_ticks"` in `items.json`) — a furnace
    /// consuming it reads this, like mining reads [`tool`](Self::tool).
    #[inline]
    pub fn fuel_burn_ticks(self) -> u16 {
        self.def().fuel_burn_ticks
    }

    /// The right-click use this item's data row declares (`"use"` in
    /// `items.json`), or `None` for items with no use of their own. The tick
    /// dispatches on the resolved [`ItemUse`], so which item fills a bucket is
    /// row data, not code.
    #[inline]
    pub fn item_use(self) -> Option<ItemUse> {
        self.def().item_use
    }

    /// Whether this item belongs to `tag`. Membership is item data — each item's
    /// [`ItemDef`](definition::ItemDef) lists its tags — so recipes can require a
    /// group (e.g. any `#llama:planks`) without naming every member, and a new
    /// item joins a group by editing its data row, never any recipe code.
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
    /// [`tool`](Self::tool) (the pickaxes, axes + shovels) is durable, and so are
    /// the shears (a durable tool that isn't a *mining* tool — it acts on a mob).
    #[inline]
    pub fn is_durable(self) -> bool {
        self.tool().is_some() || self == ItemType::Shears
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
    /// items are flat sprites pulled from [`item_sprite`](Self::item_sprite),
    /// unless they carry their own bbmodel ([`item_model`](Self::item_model)).
    #[inline]
    pub fn render_kind(self) -> ItemRenderKind {
        match self.as_block() {
            Some(block) => match block.render_shape() {
                RenderShape::Cube => ItemRenderKind::BlockCube(block),
                RenderShape::Stair => ItemRenderKind::BlockCube(block),
                RenderShape::Slab => ItemRenderKind::BlockCube(block),
                RenderShape::Cross => ItemRenderKind::Sprite(block.tiles()[0]),
                // A torch isn't a cube; it shows the full torch sprite as a flat
                // hotbar icon and an extruded sprite in-hand (like a flower), not
                // the cropped per-face tiles the in-world pole uses.
                RenderShape::Torch => ItemRenderKind::Sprite(self.item_sprite()),
                // A pane shows the flat glass tile as its icon and an extruded
                // sprite in-hand — the in-world post/arm shape only exists once
                // placed among neighbours.
                RenderShape::Pane => ItemRenderKind::Sprite(self.item_sprite()),
                // A bbmodel block renders its actual baked model everywhere it's shown.
                RenderShape::Model(kind) => ItemRenderKind::Model(kind),
                // A door shows its flat door icon (the `_door_item` art), not the
                // per-half slab tiles the in-world model uses — like the torch.
                RenderShape::Door => ItemRenderKind::Sprite(self.item_sprite()),
            },
            None => match self.item_model() {
                Some(kind) => ItemRenderKind::Model(kind),
                None => ItemRenderKind::Sprite(self.item_sprite()),
            },
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

    /// The flat atlas sprite for an item drawn as a billboard — item-only items
    /// (tools + raw drops) and the doors/torch (which place a block but show a
    /// flat icon). Read from the item's data row (`sprite` in `items.json`).
    /// Cube/cross/model block-items get their icon from the block and never call
    /// this; the stick fallback mirrors the old defensive default for a row that
    /// should carry a sprite but doesn't.
    #[inline]
    fn item_sprite(self) -> Tile {
        self.def()
            .sprite
            .unwrap_or_else(|| Tile::from_name("stick").expect("atlas has a 'stick' tile"))
    }

    /// The bbmodel an ITEM-ONLY item renders as — held, dropped, and as its slot
    /// icon — or `None` for the flat-sprite item-only items. The model counterpart
    /// of [`item_sprite`](Self::item_sprite); block-items carry their model on
    /// their block's render shape and never consult this.
    #[inline]
    fn item_model(self) -> Option<crate::block_model::BlockModelKind> {
        match self {
            ItemType::WoodenBucket => Some(crate::block_model::BlockModelKind::Bucket),
            ItemType::WaterBucket => Some(crate::block_model::BlockModelKind::WaterBucket),
            _ => None,
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
    fn attack_damage_ranges_are_ordered_and_positive() {
        // Mechanic, not the tuned numbers (which are free to change): an empty hand and
        // a non-weapon item both punch for exactly 1, and every item's range is a valid,
        // positive `lo <= hi`.
        assert_eq!(attack_damage(None), (1.0, 1.0), "fist is a deterministic 1");
        assert_eq!(
            attack_damage(Some(ItemType::Dirt)),
            (1.0, 1.0),
            "a non-weapon punches like a fist"
        );
        for &it in ItemType::all() {
            let (lo, hi) = attack_damage(Some(it));
            assert!(lo > 0.0 && lo <= hi, "{it:?}: invalid range {lo}..{hi}");
        }
        // Every diamond tool one-shots a 4-health mob (its minimum damage alone is lethal).
        for it in [
            ItemType::DiamondPickaxe,
            ItemType::DiamondAxe,
            ItemType::DiamondShovel,
        ] {
            assert!(
                attack_damage(Some(it)).0 >= 4.0,
                "a diamond tool one-shots: {it:?}"
            );
        }
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
        assert_eq!(
            ItemType::WoodenPickaxe.tool(),
            Some(Tool {
                kind: Pickaxe,
                tier: 1
            })
        );
        assert_eq!(
            ItemType::StonePickaxe.tool(),
            Some(Tool {
                kind: Pickaxe,
                tier: 2
            })
        );
        assert_eq!(
            ItemType::IronPickaxe.tool(),
            Some(Tool {
                kind: Pickaxe,
                tier: 3
            })
        );
        assert_eq!(
            ItemType::DiamondPickaxe.tool(),
            Some(Tool {
                kind: Pickaxe,
                tier: 4
            })
        );
        assert_eq!(
            ItemType::WoodenAxe.tool(),
            Some(Tool { kind: Axe, tier: 1 })
        );
        assert_eq!(
            ItemType::DiamondAxe.tool(),
            Some(Tool { kind: Axe, tier: 4 })
        );
        assert_eq!(
            ItemType::WoodenShovel.tool(),
            Some(Tool {
                kind: Shovel,
                tier: 1
            })
        );
        assert_eq!(
            ItemType::StoneShovel.tool(),
            Some(Tool {
                kind: Shovel,
                tier: 2
            })
        );
        assert_eq!(
            ItemType::IronShovel.tool(),
            Some(Tool {
                kind: Shovel,
                tier: 3
            })
        );
        assert_eq!(
            ItemType::DiamondShovel.tool(),
            Some(Tool {
                kind: Shovel,
                tier: 4
            })
        );
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
        const PLANKS: ItemTag = ItemTag::PLANKS;
        const LOGS: ItemTag = ItemTag::LOGS;
        for p in [
            ItemType::OakPlanks,
            ItemType::SprucePlanks,
            ItemType::MangrovePlanks,
        ] {
            assert!(p.has_tag(PLANKS), "{p:?}");
        }
        for log in [
            ItemType::OakLog,
            ItemType::SpruceLog,
            ItemType::BirchLog,
            ItemType::JungleLog,
            ItemType::AcaciaLog,
            ItemType::DarkOakLog,
            ItemType::CherryLog,
            ItemType::MangroveLog,
        ] {
            assert!(log.has_tag(LOGS), "{log:?}");
            assert!(!log.has_tag(PLANKS), "{log:?}");
        }
        // Sticks are neither logs nor planks.
        assert!(!ItemType::OakLog.has_tag(PLANKS));
        assert!(!ItemType::Stick.has_tag(LOGS));
        assert!(!ItemType::Stick.has_tag(PLANKS));
        // Tag names resolve from the recipe key.
        assert_eq!(ItemTag::from_key("llama:planks"), Some(PLANKS));
        assert_eq!(ItemTag::from_key("llama:logs"), Some(LOGS));
        assert_eq!(ItemTag::from_key("bogus"), None);

        // Furnace routing tags: coal is fuel; raw ores are smeltable; the products
        // are neither (so a finished ingot doesn't shift back into the furnace).
        assert!(ItemType::Coal.has_tag(ItemTag::FUEL));
        assert!(!ItemType::Coal.has_tag(ItemTag::SMELTABLE));
        assert!(ItemType::RawIron.has_tag(ItemTag::SMELTABLE));
        assert!(ItemType::RawCopper.has_tag(ItemTag::SMELTABLE));
        assert!(ItemType::Cobblestone.has_tag(ItemTag::SMELTABLE));
        assert!(!ItemType::RawIron.has_tag(ItemTag::FUEL));
        assert!(!ItemType::IronIngot.has_tag(ItemTag::SMELTABLE));
        assert!(!ItemType::IronIngot.has_tag(ItemTag::FUEL));
        assert_eq!(ItemTag::from_key("llama:fuel"), Some(ItemTag::FUEL));
        assert_eq!(
            ItemTag::from_key("llama:smeltable"),
            Some(ItemTag::SMELTABLE)
        );
    }

    #[test]
    fn render_kind_matches_render_shape() {
        for &block in Block::all() {
            let item = ItemType::from_block(block);
            match block.render_shape() {
                RenderShape::Cube => {
                    assert_eq!(
                        item.render_kind(),
                        ItemRenderKind::BlockCube(block),
                        "{block:?}"
                    );
                }
                RenderShape::Stair => {
                    assert_eq!(
                        item.render_kind(),
                        ItemRenderKind::BlockCube(block),
                        "{block:?}"
                    );
                }
                RenderShape::Slab => {
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
                        ItemRenderKind::Sprite(Tile::named("torch")),
                        "{block:?}"
                    );
                }
                RenderShape::Model(kind) => {
                    assert_eq!(item.render_kind(), ItemRenderKind::Model(kind), "{block:?}");
                }
                RenderShape::Door => {
                    assert!(
                        matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                        "{block:?} door renders as a flat sprite"
                    );
                }
                RenderShape::Pane => {
                    assert!(
                        matches!(item.render_kind(), ItemRenderKind::Sprite(_)),
                        "{block:?} pane renders as a flat sprite"
                    );
                }
            }
        }
    }

    #[test]
    fn item_only_model_item_renders_as_its_model() {
        // The bucket has no block, but must NOT fall back to a flat sprite: the
        // held / dropped / icon paths all key off the Model render kind.
        assert_eq!(ItemType::WoodenBucket.as_block(), None);
        assert!(matches!(
            ItemType::WoodenBucket.render_kind(),
            ItemRenderKind::Model(_)
        ));
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
