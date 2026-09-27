use serde::{Deserialize, Serialize};

mod accessors;
pub mod behavior;
mod data;
mod definition;
mod interaction;
pub mod item_shape_bake;
mod load;
pub mod rotation;
pub mod shape;
mod shape_kind;
mod snow;
mod sounds;
mod tags;
#[cfg(test)]
mod tests;

pub use behavior::BlockBehavior;
pub use data::light_cells;
pub use data::BlockTable;
pub use data::ENGINE_BLOCK_NAMES;
pub use data::{shape_kind_id_by_key, state_key_declared};
pub(crate) use data::{warm_views, BlockViews};
#[allow(unused_imports)]
pub use definition::ColorRamp;
pub use definition::{BlockFlags, BlockMaterial};
pub use definition::{Construction, ParticleEmitter, ParticleEmitterAnchor, RootsFace, SupportDir};
pub use interaction::builtin_claims_click;
pub use interaction::BlockInteraction;
pub use load::validate_particle_emitter;
pub(crate) use load::{load_registry, Registry as BlockRegistry};
pub use shape::BlockLightShape;
pub use shape::{
    posed_bounds, Aabb, BoxPose, CellPart, ItemBox, PosedBox, ShapeBox, ShapeFace, ShapeRenderBox,
    CROP_PLANE_DROP, CROP_PLANE_INSET,
};
pub use shape_kind::families as shape_kind_families;
pub use shape_kind::ConnectionRule;
pub use shape_kind::ItemRender;
pub(crate) use shape_kind::CUSTOM_SHAPES;
pub use shape_kind::{face_uv_turns, BlockShapeKind, ShapeFamily, ShapeKindDef, FACE_BEFORE_TURN};
pub use shape_kind::{
    full_face_at, rests_flat_on_floor, CellCodec, CellView, FullFace, NoNeighborhood, ShapeCtx,
    ShapeNeighborhood, ShapeState, NO_PART_TINT, SHAPE_STATE_MAX,
};
#[allow(unused_imports)]
pub use shape_kind::{light_aperture_face, pack_light_apertures, LIGHT_APERTURES_OPEN};
pub use shape_kind::{MeshEmitter, PlantPlanes};

pub const LIGHT_CELL_SHAPED: u32 = 1 << 31;
pub const LIGHT_CELL_DIRECT_SKY: u32 = 1 << 30;
#[allow(unused_imports)]
pub use shape_kind::{DimensionParams, ShapeParams, ShapeRender, ShapeSim};
pub use snow::{snow_cover_at, SNOW_BEDDING_REACH, SNOW_COVER_REACH};
pub use sounds::BlockSoundAction;
pub use tags::BlockTag;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Block(pub u16);

#[allow(non_upper_case_globals)]
impl Block {
    pub const Air: Block = Block(0);
    pub const Grass: Block = Block(1);
    pub const Dirt: Block = Block(2);
    pub const Stone: Block = Block(3);
    pub const Sand: Block = Block(4);
    pub const SnowLayer: Block = Block(5);
    pub const Water: Block = Block(6);
    pub const OakLog: Block = Block(7);
    pub const OakLeaves: Block = Block(8);
    pub const SpruceLog: Block = Block(9);
    pub const BirchLog: Block = Block(10);
    pub const JungleLog: Block = Block(11);
    pub const AcaciaLog: Block = Block(12);
    pub const SpruceLeaves: Block = Block(13);
    pub const BirchLeaves: Block = Block(14);
    pub const JungleLeaves: Block = Block(15);
    pub const AcaciaLeaves: Block = Block(16);
    pub const AzaleaLeaves: Block = Block(17);
    pub const RedSand: Block = Block(18);
    pub const Sandstone: Block = Block(19);
    pub const RedSandstone: Block = Block(20);
    pub const Terracotta: Block = Block(21);
    pub const WhiteTerracotta: Block = Block(22);
    pub const OrangeTerracotta: Block = Block(23);
    pub const YellowTerracotta: Block = Block(24);
    pub const BrownTerracotta: Block = Block(25);
    pub const RedTerracotta: Block = Block(26);
    pub const LightGrayTerracotta: Block = Block(27);
    pub const Podzol: Block = Block(28);
    pub const Mycelium: Block = Block(29);
    pub const CoarseDirt: Block = Block(30);
    pub const Gravel: Block = Block(31);
    pub const Clay: Block = Block(32);
    pub const Mud: Block = Block(33);
    pub const MossBlock: Block = Block(34);
    pub const SnowBlock: Block = Block(35);
    pub const PackedIce: Block = Block(36);
    pub const Ice: Block = Block(37);
    pub const Calcite: Block = Block(38);
    pub const Marble: Block = Block(39);
    pub const Tuff: Block = Block(40);
    pub const CoalOre: Block = Block(41);
    pub const IronOre: Block = Block(42);
    pub const CopperOre: Block = Block(43);
    pub const GoldOre: Block = Block(44);
    pub const DiamondOre: Block = Block(45);
    pub const Pumpkin: Block = Block(46);
    pub const Melon: Block = Block(47);
    pub const Cactus: Block = Block(48);
    pub const ShortGrass: Block = Block(49);
    pub const Fern: Block = Block(50);
    pub const Dandelion: Block = Block(51);
    pub const Poppy: Block = Block(52);
    pub const Cornflower: Block = Block(53);
    pub const Allium: Block = Block(54);
    pub const AzureBluet: Block = Block(55);
    pub const OxeyeDaisy: Block = Block(56);
    pub const RedTulip: Block = Block(57);
    pub const DeadBush: Block = Block(58);
    pub const BrownMushroom: Block = Block(59);
    pub const RedMushroom: Block = Block(60);
    pub const Cobblestone: Block = Block(61);
    pub const OakPlanks: Block = Block(62);
    pub const SprucePlanks: Block = Block(63);
    pub const BirchPlanks: Block = Block(64);
    pub const JunglePlanks: Block = Block(65);
    pub const AcaciaPlanks: Block = Block(66);
    pub const CraftingTable: Block = Block(67);
    pub const Furnace: Block = Block(68);
    pub const Chest: Block = Block(69);
    pub const Torch: Block = Block(70);
    pub const FurnitureWorkbench: Block = Block(71);
    pub const OakSapling: Block = Block(72);
    pub const SpruceSapling: Block = Block(73);
    pub const BirchSapling: Block = Block(74);
    pub const JungleSapling: Block = Block(75);
    pub const AcaciaSapling: Block = Block(76);
    pub const OakDoor: Block = Block(77);
    pub const SpruceDoor: Block = Block(78);
    pub const BirchDoor: Block = Block(79);
    pub const JungleDoor: Block = Block(80);
    pub const AcaciaDoor: Block = Block(81);
    pub const RedwoodLog: Block = Block(82);
    pub const RedwoodLeaves: Block = Block(83);
    pub const RedwoodPlanks: Block = Block(84);
    pub const RedwoodDoor: Block = Block(85);
    pub const OakStairs: Block = Block(86);
    pub const SpruceStairs: Block = Block(87);
    pub const BirchStairs: Block = Block(88);
    pub const JungleStairs: Block = Block(89);
    pub const AcaciaStairs: Block = Block(90);
    pub const RedwoodStairs: Block = Block(91);
    pub const CobblestoneStairs: Block = Block(92);
    pub const StoneStairs: Block = Block(93);
    pub const DirtStairs: Block = Block(94);
    pub const Bed: Block = Block(95);
    pub const OakSlab: Block = Block(96);
    pub const SpruceSlab: Block = Block(97);
    pub const BirchSlab: Block = Block(98);
    pub const JungleSlab: Block = Block(99);
    pub const AcaciaSlab: Block = Block(100);
    pub const RedwoodSlab: Block = Block(101);
    pub const CobblestoneSlab: Block = Block(102);
    pub const StoneSlab: Block = Block(103);
    pub const DirtSlab: Block = Block(104);
    pub const Glass: Block = Block(105);
    pub const GlassPane: Block = Block(106);
    pub const WoolBlock: Block = Block(107);
    pub const WoolStairs: Block = Block(108);
    pub const WoolSlab: Block = Block(109);
    pub const PolishedMarble: Block = Block(110);
    pub const MarbleStairs: Block = Block(111);
    pub const MarbleSlab: Block = Block(112);
    pub const PolishedMarbleStairs: Block = Block(113);
    pub const PolishedMarbleSlab: Block = Block(114);
    pub const Ladder: Block = Block(115);
    // Sapling growth stages 1 and 2; stage 0 is the base sapling row above. They look like
    // the base row. The `sapling` behaviour walks the `next_stage` chain, and the last row's
    // `grows_into` names the tree (see `world::sapling`).
    pub const OakSapling1: Block = Block(116);
    pub const OakSapling2: Block = Block(117);
    pub const SpruceSapling1: Block = Block(118);
    pub const SpruceSapling2: Block = Block(119);
    pub const BirchSapling1: Block = Block(120);
    pub const BirchSapling2: Block = Block(121);
    pub const JungleSapling1: Block = Block(122);
    pub const JungleSapling2: Block = Block(123);
    pub const AcaciaSapling1: Block = Block(124);
    pub const AcaciaSapling2: Block = Block(125);
    // Furnace lit skin - same trick as sapling stages, applied to a machine.
    // Burning swaps the `furnace`/`furnace_lit` rows so the lit face and light
    // travel with block identity through save and replication.
    // Counters sit in the `Furnace` block-entity; the swap keeps the sibling entity
    // maps, see `World::tick_furnaces`. Not obtainable: no item row links it, and it
    // drops the furnace item like the unlit row.
    pub const FurnaceLit: Block = Block(126);
    // Ladder's non-default wall facings, same trick as sapling stages but on
    // an oriented panel. Which wall it's on is block identity: `petramond:ladder`
    // is the north row, `panel_facing` gives each row's facing, and `facing_rows`
    // maps a placement facing to its sibling. The facing rides normal block-id
    // save/replication; the ladder isn't a block entity, so it never touches the
    // entity-facing map. Not obtainable: no item rows link these, and all four
    // drop the same ladder item.
    pub const LadderSouth: Block = Block(127);
    pub const LadderWest: Block = Block(128);
    pub const LadderEast: Block = Block(129);
    pub const OakFence: Block = Block(130);
    pub const SpruceFence: Block = Block(131);
    pub const BirchFence: Block = Block(132);
    pub const JungleFence: Block = Block(133);
    pub const AcaciaFence: Block = Block(134);
    pub const RedwoodFence: Block = Block(135);
    pub const PebblesSmall: Block = Block(136);
    pub const PebblesMedium: Block = Block(137);
    pub const PebblesLarge: Block = Block(138);
    pub const FallenBranch: Block = Block(139);
    pub const Hemp: Block = Block(140);
    pub const FallenBranch2: Block = Block(141);
    pub const FallenBranch3: Block = Block(142);
    pub const Lava: Block = Block(143);
    pub const StoneBricks: Block = Block(144);
    pub const StoneBricksStairs: Block = Block(145);
    pub const StoneBricksSlab: Block = Block(146);
    pub const ChiselingStation: Block = Block(147);
    pub const OakTrapdoor: Block = Block(148);
    pub const SpruceTrapdoor: Block = Block(149);
    pub const BirchTrapdoor: Block = Block(150);
    pub const JungleTrapdoor: Block = Block(151);
    pub const AcaciaTrapdoor: Block = Block(152);
    pub const RedwoodTrapdoor: Block = Block(153);
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_BLOCK_NAMES.get(self.0 as usize) {
            Some(name) => write!(f, "Block({name})"),
            None => write!(f, "Block(#{})", self.0),
        }
    }
}

impl Serialize for Block {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match crate::registry::names().blocks.name(self.0) {
            Some(name) => s.serialize_str(name),
            None => Err(serde::ser::Error::custom(format!(
                "block id {} is not registered",
                self.0
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for Block {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = std::borrow::Cow::<str>::deserialize(d)?;
        crate::registry::names()
            .blocks
            .id(&name)
            .map(Block)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown block '{name}'")))
    }
}

pub const TINT_KV_KEY: &str = "petramond:tint";

const PART_KEY_SEP: char = '#';

pub fn part_kv_key(key: &str, part: CellPart) -> String {
    if part == 0 {
        key.to_owned()
    } else {
        format!("{key}{PART_KEY_SEP}{part}")
    }
}

pub fn split_part_kv_key(stored: &str) -> (&str, CellPart) {
    match stored.rsplit_once(PART_KEY_SEP) {
        Some((base, suffix)) => match suffix.parse::<CellPart>() {
            Ok(part) if part != 0 => (base, part),
            _ => (stored, 0),
        },
        None => (stored, 0),
    }
}

#[inline]
pub fn kv_key_affects_mesh(key: &str) -> bool {
    let base = split_part_kv_key(key).0;
    base == TINT_KV_KEY || base == crate::block_model::PARTS_KV_KEY
}
