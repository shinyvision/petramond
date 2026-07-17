//! Item model: the inventory-space counterpart of `Block`.
//!
//! An item that places a block declares it in its `items.json` row (`"block":
//! "<registry name>"`, engine and pack rows alike): `as_block` reads that row
//! field and `from_block` reads the dense reverse LUT inverted from it at load
//! (see `data`). Item-only items (tools, raw drops) carry no link — `as_block`
//! returns `None` and they render as flat sprites (or their row's `"model"`
//! bbmodel). Engine ids stay append-only; pack items register past them (see
//! [`crate::registry`]).
//!
//! Per-item static data (`key`, `name`, `max_stack_size`) lives in an id-ordered
//! table loaded from `assets/items.json`, mirroring `block/data.rs`. The `key` is
//! the stable recipe identity; `name` is display-only. Behaviour derivable from the
//! underlying `Block` (`render_kind` for block-items) is computed via `Block`.

mod accessors;
mod data;
mod definition;
mod drops;
mod food;
mod load;
mod reaction;
mod render;
mod stack;
mod tags;
#[cfg(test)]
mod tests;
mod tool;
mod uses;

pub(crate) use data::ENGINE_ITEM_NAMES;
pub use drops::{Drop, DropSpec};
pub use food::FoodDef;
pub use reaction::{DroppedReaction, ReactionEnvironment};
pub use render::{HeldPose, ItemRenderKind};
pub use stack::ItemStack;
pub use tags::ItemTag;
#[allow(unused_imports)]
pub use tool::FIST_DAMAGE;
pub use tool::{attack_damage, Tool, ToolKind};
pub use uses::{ItemUse, UseRay};

/// A registered item, identified by its opaque runtime id. Engine items own
/// the low ids in a compiled, frozen order (the named consts below — save
/// palettes depend on those ids/names never moving); mod packs register
/// additional ids at load through namespaced `items.json` rows (see
/// [`crate::registry`]). Serde carries an item as its registered NAME string.
///
/// An item links to the block it places through its row's `block` field;
/// `from_block` / `as_block` are table lookups over those links.
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
    pub const Snowball: ItemType = ItemType(5);
    pub const Water: ItemType = ItemType(6);
    pub const OakLog: ItemType = ItemType(7);
    pub const OakLeaves: ItemType = ItemType(8);
    pub const SpruceLog: ItemType = ItemType(9);
    pub const BirchLog: ItemType = ItemType(10);
    pub const JungleLog: ItemType = ItemType(11);
    pub const AcaciaLog: ItemType = ItemType(12);
    pub const SpruceLeaves: ItemType = ItemType(13);
    pub const BirchLeaves: ItemType = ItemType(14);
    pub const JungleLeaves: ItemType = ItemType(15);
    pub const AcaciaLeaves: ItemType = ItemType(16);
    pub const AzaleaLeaves: ItemType = ItemType(17);
    pub const RedSand: ItemType = ItemType(18);
    pub const Sandstone: ItemType = ItemType(19);
    pub const RedSandstone: ItemType = ItemType(20);
    pub const Terracotta: ItemType = ItemType(21);
    pub const WhiteTerracotta: ItemType = ItemType(22);
    pub const OrangeTerracotta: ItemType = ItemType(23);
    pub const YellowTerracotta: ItemType = ItemType(24);
    pub const BrownTerracotta: ItemType = ItemType(25);
    pub const RedTerracotta: ItemType = ItemType(26);
    pub const LightGrayTerracotta: ItemType = ItemType(27);
    pub const Podzol: ItemType = ItemType(28);
    pub const Mycelium: ItemType = ItemType(29);
    pub const CoarseDirt: ItemType = ItemType(30);
    pub const Gravel: ItemType = ItemType(31);
    pub const Clay: ItemType = ItemType(32);
    pub const Mud: ItemType = ItemType(33);
    pub const MossBlock: ItemType = ItemType(34);
    pub const SnowBlock: ItemType = ItemType(35);
    pub const PackedIce: ItemType = ItemType(36);
    pub const Ice: ItemType = ItemType(37);
    pub const Calcite: ItemType = ItemType(38);
    pub const Marble: ItemType = ItemType(39);
    pub const Tuff: ItemType = ItemType(40);
    pub const CoalOre: ItemType = ItemType(41);
    pub const IronOre: ItemType = ItemType(42);
    pub const CopperOre: ItemType = ItemType(43);
    pub const GoldOre: ItemType = ItemType(44);
    pub const DiamondOre: ItemType = ItemType(45);
    pub const Pumpkin: ItemType = ItemType(46);
    pub const Melon: ItemType = ItemType(47);
    pub const Cactus: ItemType = ItemType(48);
    pub const ShortGrass: ItemType = ItemType(49);
    pub const Fern: ItemType = ItemType(50);
    pub const Dandelion: ItemType = ItemType(51);
    pub const Poppy: ItemType = ItemType(52);
    pub const Cornflower: ItemType = ItemType(53);
    pub const Allium: ItemType = ItemType(54);
    pub const AzureBluet: ItemType = ItemType(55);
    pub const OxeyeDaisy: ItemType = ItemType(56);
    pub const RedTulip: ItemType = ItemType(57);
    pub const DeadBush: ItemType = ItemType(58);
    pub const BrownMushroom: ItemType = ItemType(59);
    pub const RedMushroom: ItemType = ItemType(60);
    pub const Cobblestone: ItemType = ItemType(61);
    pub const OakPlanks: ItemType = ItemType(62);
    pub const SprucePlanks: ItemType = ItemType(63);
    pub const BirchPlanks: ItemType = ItemType(64);
    pub const JunglePlanks: ItemType = ItemType(65);
    pub const AcaciaPlanks: ItemType = ItemType(66);
    pub const CraftingTable: ItemType = ItemType(67);
    pub const Stick: ItemType = ItemType(68);
    pub const WoodenPickaxe: ItemType = ItemType(69);
    pub const StonePickaxe: ItemType = ItemType(70);
    pub const RawIron: ItemType = ItemType(71);
    pub const RawCopper: ItemType = ItemType(72);
    pub const Coal: ItemType = ItemType(73);
    pub const IronIngot: ItemType = ItemType(74);
    pub const CopperIngot: ItemType = ItemType(75);
    pub const Furnace: ItemType = ItemType(76);
    pub const Chest: ItemType = ItemType(77);
    pub const Torch: ItemType = ItemType(78);
    pub const Diamond: ItemType = ItemType(79);
    pub const RawGold: ItemType = ItemType(80);
    pub const GoldIngot: ItemType = ItemType(81);
    pub const WoodenAxe: ItemType = ItemType(82);
    pub const StoneAxe: ItemType = ItemType(83);
    pub const IronAxe: ItemType = ItemType(84);
    pub const DiamondAxe: ItemType = ItemType(85);
    pub const IronPickaxe: ItemType = ItemType(86);
    pub const DiamondPickaxe: ItemType = ItemType(87);
    pub const WoodenShovel: ItemType = ItemType(88);
    pub const StoneShovel: ItemType = ItemType(89);
    pub const IronShovel: ItemType = ItemType(90);
    pub const DiamondShovel: ItemType = ItemType(91);
    pub const FurnitureWorkbench: ItemType = ItemType(92);
    pub const OakSapling: ItemType = ItemType(93);
    pub const SpruceSapling: ItemType = ItemType(94);
    pub const BirchSapling: ItemType = ItemType(95);
    pub const JungleSapling: ItemType = ItemType(96);
    pub const AcaciaSapling: ItemType = ItemType(97);
    pub const OakDoor: ItemType = ItemType(98);
    pub const SpruceDoor: ItemType = ItemType(99);
    pub const BirchDoor: ItemType = ItemType(100);
    pub const JungleDoor: ItemType = ItemType(101);
    pub const AcaciaDoor: ItemType = ItemType(102);
    pub const RedwoodLog: ItemType = ItemType(103);
    pub const RedwoodLeaves: ItemType = ItemType(104);
    pub const RedwoodPlanks: ItemType = ItemType(105);
    pub const RedwoodDoor: ItemType = ItemType(106);
    pub const OakStairs: ItemType = ItemType(107);
    pub const SpruceStairs: ItemType = ItemType(108);
    pub const BirchStairs: ItemType = ItemType(109);
    pub const JungleStairs: ItemType = ItemType(110);
    pub const AcaciaStairs: ItemType = ItemType(111);
    pub const RedwoodStairs: ItemType = ItemType(112);
    pub const CobblestoneStairs: ItemType = ItemType(113);
    pub const StoneStairs: ItemType = ItemType(114);
    pub const DirtStairs: ItemType = ItemType(115);
    pub const WoodenBucket: ItemType = ItemType(116);
    pub const WaterBucket: ItemType = ItemType(117);
    pub const Shears: ItemType = ItemType(118);
    pub const Wool: ItemType = ItemType(119);
    pub const BedFrame: ItemType = ItemType(120);
    pub const Bed: ItemType = ItemType(121);
    pub const OakSlab: ItemType = ItemType(122);
    pub const SpruceSlab: ItemType = ItemType(123);
    pub const BirchSlab: ItemType = ItemType(124);
    pub const JungleSlab: ItemType = ItemType(125);
    pub const AcaciaSlab: ItemType = ItemType(126);
    pub const RedwoodSlab: ItemType = ItemType(127);
    pub const CobblestoneSlab: ItemType = ItemType(128);
    pub const StoneSlab: ItemType = ItemType(129);
    pub const DirtSlab: ItemType = ItemType(130);
    pub const Glass: ItemType = ItemType(131);
    pub const GlassPane: ItemType = ItemType(132);
    pub const WoolBlock: ItemType = ItemType(133);
    pub const WoolStairs: ItemType = ItemType(134);
    pub const WoolSlab: ItemType = ItemType(135);
    pub const PolishedMarble: ItemType = ItemType(136);
    pub const MarbleStairs: ItemType = ItemType(137);
    pub const MarbleSlab: ItemType = ItemType(138);
    pub const PolishedMarbleStairs: ItemType = ItemType(139);
    pub const PolishedMarbleSlab: ItemType = ItemType(140);
    pub const Ladder: ItemType = ItemType(141);
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
