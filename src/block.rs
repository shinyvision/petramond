//! Block registry + per-face tile mapping.

use serde::{Deserialize, Serialize};

use crate::atlas::Tile;
use crate::audio::Sound;
use crate::block_model::BlockModelKind;
use crate::item::{DropSpec, ItemType, ToolKind};

pub mod behavior;
mod data;
mod definition;
mod load;
mod sounds;

pub use behavior::BlockBehavior;
pub(crate) use data::ENGINE_BLOCK_NAMES;
pub(crate) use definition::BlockMaterial;
// ColorRamp rides the public `ParticleEmitter::color_ramp` field; only tests
// currently name the type, so the lib build sees the re-export as unused.
#[allow(unused_imports)]
pub use definition::ColorRamp;
pub use definition::{ParticleEmitter, ParticleEmitterAnchor};
pub(crate) use load::validate_particle_emitter;
pub use sounds::BlockSoundAction;

/// A registered block, identified by its opaque runtime id. Engine blocks own
/// the low ids in a compiled, frozen order (the named consts below — worldgen
/// parity and save palettes depend on those ids never moving); mod packs
/// register additional ids at load through namespaced `blocks.json` rows (see
/// [`crate::registry`]). Serde carries a block as its registered NAME string,
/// so persisted data never depends on numeric ids.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Block(pub u8);

/// Engine block consts, named like the enum variants they replaced so every
/// existing `Block::OakLog` expression and match pattern keeps compiling
/// (the derives keep the newtype a structural-match type).
#[allow(non_upper_case_globals)]
impl Block {
    pub const Air: Block = Block(0);
    pub const Grass: Block = Block(1);
    pub const Dirt: Block = Block(2);
    pub const Stone: Block = Block(3);
    pub const Sand: Block = Block(4);
    pub const Snow: Block = Block(5);
    pub const Water: Block = Block(6);
    pub const OakLog: Block = Block(7);
    pub const OakLeaves: Block = Block(8);
    pub const SpruceLog: Block = Block(9);
    pub const BirchLog: Block = Block(10);
    pub const JungleLog: Block = Block(11);
    pub const AcaciaLog: Block = Block(12);
    pub const DarkOakLog: Block = Block(13);
    pub const CherryLog: Block = Block(14);
    pub const MangroveLog: Block = Block(15);
    pub const SpruceLeaves: Block = Block(16);
    pub const BirchLeaves: Block = Block(17);
    pub const JungleLeaves: Block = Block(18);
    pub const AcaciaLeaves: Block = Block(19);
    pub const DarkOakLeaves: Block = Block(20);
    pub const MangroveLeaves: Block = Block(21);
    pub const CherryLeaves: Block = Block(22);
    pub const AzaleaLeaves: Block = Block(23);
    pub const RedSand: Block = Block(24);
    pub const Sandstone: Block = Block(25);
    pub const RedSandstone: Block = Block(26);
    pub const Terracotta: Block = Block(27);
    pub const WhiteTerracotta: Block = Block(28);
    pub const OrangeTerracotta: Block = Block(29);
    pub const YellowTerracotta: Block = Block(30);
    pub const BrownTerracotta: Block = Block(31);
    pub const RedTerracotta: Block = Block(32);
    pub const LightGrayTerracotta: Block = Block(33);
    pub const Podzol: Block = Block(34);
    pub const Mycelium: Block = Block(35);
    pub const CoarseDirt: Block = Block(36);
    pub const Gravel: Block = Block(37);
    pub const Clay: Block = Block(38);
    pub const Mud: Block = Block(39);
    pub const MossBlock: Block = Block(40);
    pub const SnowBlock: Block = Block(41);
    pub const PackedIce: Block = Block(42);
    pub const Ice: Block = Block(43);
    pub const Calcite: Block = Block(44);
    pub const Marble: Block = Block(45);
    pub const Tuff: Block = Block(46);
    pub const CoalOre: Block = Block(47);
    pub const IronOre: Block = Block(48);
    pub const CopperOre: Block = Block(49);
    pub const GoldOre: Block = Block(50);
    pub const DiamondOre: Block = Block(51);
    pub const Pumpkin: Block = Block(52);
    pub const Melon: Block = Block(53);
    pub const Cactus: Block = Block(54);
    pub const ShortGrass: Block = Block(55);
    pub const Fern: Block = Block(56);
    pub const Dandelion: Block = Block(57);
    pub const Poppy: Block = Block(58);
    pub const Cornflower: Block = Block(59);
    pub const Allium: Block = Block(60);
    pub const AzureBluet: Block = Block(61);
    pub const OxeyeDaisy: Block = Block(62);
    pub const RedTulip: Block = Block(63);
    pub const DeadBush: Block = Block(64);
    pub const BrownMushroom: Block = Block(65);
    pub const RedMushroom: Block = Block(66);
    pub const Cobblestone: Block = Block(67);
    pub const OakPlanks: Block = Block(68);
    pub const SprucePlanks: Block = Block(69);
    pub const BirchPlanks: Block = Block(70);
    pub const JunglePlanks: Block = Block(71);
    pub const AcaciaPlanks: Block = Block(72);
    pub const DarkOakPlanks: Block = Block(73);
    pub const CherryPlanks: Block = Block(74);
    pub const MangrovePlanks: Block = Block(75);
    pub const CraftingTable: Block = Block(76);
    pub const Furnace: Block = Block(77);
    pub const Chest: Block = Block(78);
    pub const Torch: Block = Block(79);
    pub const FurnitureWorkbench: Block = Block(80);
    pub const OakSapling: Block = Block(81);
    pub const SpruceSapling: Block = Block(82);
    pub const BirchSapling: Block = Block(83);
    pub const JungleSapling: Block = Block(84);
    pub const AcaciaSapling: Block = Block(85);
    pub const DarkOakSapling: Block = Block(86);
    pub const CherrySapling: Block = Block(87);
    pub const OakDoor: Block = Block(88);
    pub const SpruceDoor: Block = Block(89);
    pub const BirchDoor: Block = Block(90);
    pub const JungleDoor: Block = Block(91);
    pub const AcaciaDoor: Block = Block(92);
    pub const DarkOakDoor: Block = Block(93);
    pub const CherryDoor: Block = Block(94);
    pub const MangroveDoor: Block = Block(95);
    pub const RedwoodLog: Block = Block(96);
    pub const RedwoodLeaves: Block = Block(97);
    pub const RedwoodPlanks: Block = Block(98);
    pub const RedwoodDoor: Block = Block(99);
    pub const OakStairs: Block = Block(100);
    pub const SpruceStairs: Block = Block(101);
    pub const BirchStairs: Block = Block(102);
    pub const JungleStairs: Block = Block(103);
    pub const AcaciaStairs: Block = Block(104);
    pub const DarkOakStairs: Block = Block(105);
    pub const CherryStairs: Block = Block(106);
    pub const MangroveStairs: Block = Block(107);
    pub const RedwoodStairs: Block = Block(108);
    pub const CobblestoneStairs: Block = Block(109);
    pub const StoneStairs: Block = Block(110);
    pub const DirtStairs: Block = Block(111);
    pub const BedFrame: Block = Block(112);
    pub const Bed: Block = Block(113);
    pub const OakSlab: Block = Block(114);
    pub const SpruceSlab: Block = Block(115);
    pub const BirchSlab: Block = Block(116);
    pub const JungleSlab: Block = Block(117);
    pub const AcaciaSlab: Block = Block(118);
    pub const DarkOakSlab: Block = Block(119);
    pub const CherrySlab: Block = Block(120);
    pub const MangroveSlab: Block = Block(121);
    pub const RedwoodSlab: Block = Block(122);
    pub const CobblestoneSlab: Block = Block(123);
    pub const StoneSlab: Block = Block(124);
    pub const DirtSlab: Block = Block(125);
    pub const Glass: Block = Block(126);
    pub const GlassPane: Block = Block(127);
}

impl std::fmt::Debug for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Engine names come from the compiled table (never a lazy registry —
        // Debug must work mid-bootstrap); dynamic ids print numerically.
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

/// Secondary-use capability declared by a block's data row. This answers only
/// "what use action is available"; the tick-side gameplay code still applies the
/// concrete world mutation or menu request. Parsed from the row's
/// `interaction` field: a bare action name, or `{"open_gui": "mod_id:name"}`
/// for a mod-defined GUI (see `block::load`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockInteraction {
    None,
    OpenCraftingTable,
    OpenFurnace,
    OpenChest,
    OpenFurnitureWorkbench,
    ToggleDoor,
    /// Right-click puts the player to sleep in this block (a bed): sets the
    /// spawn point beside it and starts the sleep fade (see `game::bed`).
    Sleep,
    /// Right-click opens the mod GUI registered under this kind (Phase 5).
    OpenModGui(crate::gui::GuiKind),
}

/// A named category a block belongs to — a PROPERTY OF BLOCKS, exactly as
/// [`ItemTag`](crate::item::ItemTag) is a property of items. Each block lists its
/// tags in its [`BlockDef`](definition::BlockDef) data row and code asks via
/// [`Block::has_tag`]; keeping membership in the data means a block joins a
/// category by editing its row, never a `match` in this file. Tags answer "what
/// *is* this block" (categorisation); [`behavior`] answers "what does it *do*".
///
/// The vocabulary is OPEN: engine tags are the named consts below (bare
/// snake_case in `blocks.json`); a pack introduces its own tag by listing a
/// namespaced `mod_id:name`, interned at load (see [`crate::registry::TagTable`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockTag(u8);

/// Engine block-tag names, id-ordered: `BLOCK_TAGS.resolve(ENGINE[i]) == i`
/// matches the consts on [`BlockTag`].
static BLOCK_TAGS: crate::registry::TagTable = crate::registry::TagTable::new(&[
    "leaves",
    "log",
    "terrain",
    "no_grass_decay",
    "fragile",
    "replaceable",
    "soil",
    "sand",
    "roots_in_soil",
    "roots_in_sand",
    "roots_in_stone",
    "no_pane_connect",
]);

impl BlockTag {
    /// Any tree-leaves block: takes random ticks and decays when cut off, and
    /// counts as the support that keeps an adjacent leaf alive.
    pub const LEAVES: BlockTag = BlockTag(0);
    /// Any tree-log block: counts as support that keeps adjacent leaves alive.
    pub const LOG: BlockTag = BlockTag(1);
    /// Natural ground surface — the bare-terrain set (stone/dirt/grass/sand/snow),
    /// excluding tree parts and built blocks. Worldgen audits measure overhangs /
    /// floating debris against it (see `worldgen::audit`).
    pub const TERRAIN: BlockTag = BlockTag(2);
    /// A solid block that nonetheless does NOT smother the grass directly below it:
    /// grass under it survives instead of dying back to dirt, and bare dirt can
    /// still green over into grass beneath it. Leaves carry this — a leaf canopy
    /// lets grass live, unlike a solid roof of stone or planks (see `behavior::grass`).
    pub const NO_GRASS_DECAY: BlockTag = BlockTag(3);
    /// A delicate block that cannot stand on its own: it shatters — dropping and
    /// bursting as if a player hand-broke it — the instant it loses the support it
    /// rests on (a plant whose ground is dug away; a wall-torch whose wall is mined),
    /// and it is washed away when water flows or falls into its cell. The reactive
    /// break is the [`FRAGILE`](behavior::FRAGILE) behaviour; this tag is the
    /// *categorisation* the water sim reads to treat the cell as one it may flow
    /// into. Carried by the cross-plants (grass, ferns, flowers, mushrooms), the
    /// cactus, and the torch — every block whose row points at `behavior::FRAGILE`.
    pub const FRAGILE: BlockTag = BlockTag(4);
    /// A cell a placement may overwrite in place: building into it — or right-clicking
    /// it while holding a block — replaces it with no drop, as if it were empty.
    /// Air and water carry it, as does walk-through grassy foliage (short grass, fern,
    /// dead bush). The one predicate is [`Block::is_replaceable`].
    pub const REPLACEABLE: BlockTag = BlockTag(5);
    /// Fertile ground — grass and dirt — that small plants take root in: the
    /// *category of ground* the cross-plants ([`ROOTS_IN_SOIL`](BlockTag::ROOTS_IN_SOIL))
    /// require beneath them to be placed. Read via [`Block::required_ground`].
    pub const SOIL: BlockTag = BlockTag(6);
    /// Loose sandy ground — sand and red sand — that desert flora root in: the
    /// substrate [`ROOTS_IN_SAND`](BlockTag::ROOTS_IN_SAND) blocks (cactus, dead
    /// bush) require beneath them.
    pub const SAND: BlockTag = BlockTag(7);
    /// A plant that may only be PLACED on [`SOIL`](BlockTag::SOIL) (grass or dirt) —
    /// the cross-plants: flowers, ferns, short grass. The placement gate
    /// (`game::try_place`) reads it through [`Block::required_ground`]; staying rooted
    /// once placed is the separate physical job of [`FRAGILE`](behavior::FRAGILE).
    pub const ROOTS_IN_SOIL: BlockTag = BlockTag(8);
    /// A plant that may only be PLACED on [`SAND`](BlockTag::SAND) (sand or red sand)
    /// — the desert flora: cactus and dead bush.
    pub const ROOTS_IN_SAND: BlockTag = BlockTag(9);
    /// A plant that may also be PLACED on stone — any block of the
    /// [`BlockMaterial::Stone`] class (stone, cobblestone, sandstone, granite…). The
    /// mushrooms carry it ALONGSIDE [`ROOTS_IN_SOIL`](BlockTag::ROOTS_IN_SOIL), so they
    /// take to soil OR stone; the `RootsIn*` tags combine (see [`Block::can_root_on`]).
    pub const ROOTS_IN_STONE: BlockTag = BlockTag(10);
    /// A block a glass pane never joins toward, even though its row would
    /// otherwise qualify — cube rows whose REAL shape is not the full cell (the
    /// inset cactus and chest). Panes connect by meeting a complete 1x1 face
    /// (see `crate::pane`); this tag is the per-row opt-out for blocks whose
    /// cube row overstates their geometry.
    pub const NO_PANE_CONNECT: BlockTag = BlockTag(11);

    /// Resolve a `blocks.json` row tag name (see [`crate::registry::TagTable`]).
    pub(crate) fn resolve(name: &str) -> Result<BlockTag, String> {
        BLOCK_TAGS.resolve(name).map(BlockTag)
    }
}

/// How a block's geometry is meshed. `Cube` is the standard 6-face box; `Cross`
/// is an X of two diagonal billboard quads (grass, ferns, flowers, mushrooms);
/// `Torch` is a thin pole (a small box) standing on the floor or tilted against a
/// wall, with its orientation read from the chunk's torch map (see `mesh::torch`);
/// `Model` is a data-driven Blockbench block ([`BlockModelKind`]) — NOT chunk-meshed
/// (like the chest it is drawn each frame as a placed model, see
/// `render::placed_model`), with its own texture, collision and selection baked from
/// the `.bbmodel` (see [`crate::block_model`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderShape {
    Cube,
    Cross,
    Torch,
    /// A chunk-meshed directional stair, with the low side facing the player when
    /// placed. Its per-cell facing lives in the section's stair-facing map; collision,
    /// selection, and meshing resolve straight/corner boxes through `crate::stair`.
    Stair,
    /// A chunk-meshed half-cell slab. Its per-cell state stores the split axis and up
    /// to two material-bearing layers, so a cell can hold mixed slabs without adding a
    /// registry row for every material pair.
    Slab,
    /// A chunk-meshed glass pane: a thin full-height post that grows arms toward
    /// the horizontal neighbours it connects to. The connection mask is NOT
    /// stored state — it is resolved from the current neighbours wherever the
    /// shape is needed (collision, selection, meshing), like stair corners. See
    /// `crate::pane` for the connection rules and boxes.
    Pane,
    Model(BlockModelKind),
    /// A wooden door: a 2-tall thin slab on a cell edge. Like the chest it is NOT
    /// chunk-meshed — it is drawn each frame as a dynamic hinged model (see
    /// `render::door_model`) so the leaf can swing smoothly. Its facing + open +
    /// which-half state lives in the chunk door map; the per-cell collision and
    /// selection boxes are resolved position-aware in `world::door` from that state
    /// (see [`crate::door`]). The mesher skips a door cell, exactly like a chest.
    Door,
}

/// How a block participates in light propagation. This is the render/collision-neutral
/// shape category that `world::light` consumes; per-cell state, such as stair facing,
/// still lives in the section and is interpreted by the lighting shape layer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BlockLightShape {
    Open,
    OpaqueCube,
    Stair,
    Slab,
}

/// One axis-aligned box of a block's collision shape, in CELL-LOCAL coordinates
/// (`0.0..1.0` per axis). A block's full shape is a *list* of these (see
/// [`Block::collision_boxes`]) — one for a full cube or the inset chest, several for
/// shapes like stairs. The player collides via a swept-AABB over them, and the
/// selection outline + break overlay derive from their union.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Block {
    /// Every registered block — engine first (frozen ids), then pack-registered
    /// blocks in load order — as an id-ordered slice (`all()[id] == Block(id)`).
    pub fn all() -> &'static [Block] {
        data::all()
    }

    /// Mesh geometry kind — cube / cross-plant / torch — a per-row [`BlockDef`]
    /// field (see [`RenderShape`]): cross-model plants render as billboards, a torch
    /// as a thin pole, everything else as a full cube.
    #[inline]
    pub fn render_shape(self) -> RenderShape {
        self.def().shape
    }

    #[inline]
    pub(crate) fn light_shape(self) -> BlockLightShape {
        if self.is_opaque() {
            return BlockLightShape::OpaqueCube;
        }
        match self.def().shape {
            RenderShape::Stair => BlockLightShape::Stair,
            RenderShape::Slab => BlockLightShape::Slab,
            _ => BlockLightShape::Open,
        }
    }

    /// The block's collision shape: cell-local AABBs (`0.0..1.0`), a per-row
    /// [`BlockDef`] field. Empty = no collision: air, water, walk-through plants,
    /// and the torch (selectable by its custom pole shape yet stepped through — see
    /// `player::interaction`). One unit box
    /// for an ordinary full cube; the chest is a single inset box; future
    /// stairs/slabs list several. The single source of truth for player collision
    /// AND — via the union — the selection outline + break overlay
    /// ([`visual_aabb`](Self::visual_aabb)).
    #[inline]
    pub fn collision_boxes(self) -> &'static [Aabb] {
        // A bbmodel block's collision comes from its model — see `block_model` — not the
        // data row. This position-LESS accessor answers the footprint-origin cell (the
        // whole block for a single-cell model); a multi-block's per-cell collision is
        // answered by [`World::collision_boxes_at`](crate::world::World::collision_boxes_at),
        // which knows the cell's offset.
        if let RenderShape::Model(kind) = self.def().shape {
            return crate::block_model::collision_boxes(kind, [0, 0, 0]);
        }
        if self.def().shape == RenderShape::Stair {
            return crate::stair::boxes(crate::block_model::DEFAULT_MODEL_FACING);
        }
        if self.def().shape == RenderShape::Slab {
            return crate::slab::default_boxes();
        }
        if self.def().shape == RenderShape::Pane {
            return crate::pane::boxes_for_mask(0);
        }
        self.def().collision
    }

    /// Whether this block physically obstructs movement — i.e. has any collision
    /// box. The single predicate for "can an entity rest on / be stopped by this
    /// cell", derived from [`collision_boxes`](Self::collision_boxes) (the physics
    /// source of truth) rather than [`is_solid`](Self::is_solid) (material solidity):
    /// they coincide today, but collision is what governs movement, so a future
    /// partial block (slab/fence) could obstruct without being a full solid.
    #[inline]
    pub fn blocks_movement(self) -> bool {
        !self.collision_boxes().is_empty()
    }

    /// The visual bounding box (cell-local) for a non-full-cube block — the union of
    /// its [`collision_boxes`](Self::collision_boxes) — used for the selection outline
    /// and the break-crack overlay so they hug the block's actual shape. `None` = an
    /// ordinary full cube (or a non-colliding block), which needs no special outline.
    #[inline]
    pub fn visual_aabb(self) -> Option<([f32; 3], [f32; 3])> {
        // A bbmodel block outlines its MODEL's selection box (raycast target + black
        // wireframe + break overlay), independent of its collision — so a walk-through
        // (no-collision) model block is still selectable. Position-LESS: answers the
        // footprint-origin cell; the per-cell outline of a multi-block is resolved by
        // [`World::selection_box_at`](crate::world::World::selection_box_at). See `block_model`.
        if let RenderShape::Model(kind) = self.def().shape {
            return crate::block_model::selection_aabb(kind, [0, 0, 0]);
        }
        let boxes = self.collision_boxes();
        if boxes.is_empty() {
            return None;
        }
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for b in boxes {
            for i in 0..3 {
                mn[i] = mn[i].min(b.min[i]);
                mx[i] = mx[i].max(b.max[i]);
            }
        }
        // A full unit cube needs no special outline (the default selection is a cube).
        if mn == [0.0; 3] && mx == [1.0; 3] {
            None
        } else {
            Some((mn, mx))
        }
    }

    #[inline]
    pub const fn id(self) -> u8 {
        self.0
    }

    #[inline]
    pub fn from_id(id: u8) -> Block {
        data::from_id(id)
    }

    #[inline]
    pub fn is_solid(self) -> bool {
        data::flags(self.id()).is_solid()
    }

    /// Whether this block carries `tag` (see [`BlockTag`]) — the one tag query.
    /// The named predicates below are thin wrappers over it so call sites read
    /// well; membership itself lives per-row in the data table.
    #[inline]
    pub fn has_tag(self, tag: BlockTag) -> bool {
        self.def().tags.contains(&tag)
    }

    /// Whether this is a natural terrain-solid block: the bare-ground set
    /// (`Stone`, `Dirt`, `Grass`, `Sand`, `Snow`) that makes up the land surface,
    /// EXCLUDING tree logs/leaves and built blocks. Worldgen audits use this to
    /// measure terrain overhangs/floating debris without tree canopy swamping the
    /// signal (see `worldgen::audit`). Narrower than [`is_solid`](Self::is_solid).
    #[inline]
    pub fn is_terrain_solid(self) -> bool {
        self.has_tag(BlockTag::TERRAIN)
    }

    /// Whether this is any tree-leaves variant. Leaves form the canopy: they take
    /// random ticks and decay when cut off from wood, and are the support a
    /// neighbouring leaf looks for (alongside logs). See [`behavior`].
    #[inline]
    pub fn is_leaves(self) -> bool {
        self.has_tag(BlockTag::LEAVES)
    }

    /// Whether this is any tree-log variant. A log keeps nearby leaves alive: a
    /// leaf with no log within a few steps (through leaves) decays — see the flood
    /// in [`behavior`].
    #[inline]
    pub fn is_log(self) -> bool {
        self.has_tag(BlockTag::LOG)
    }

    /// Whether this is water (source or flowing — one block id, the flow is metadata).
    /// Water has no collision, so mobs sink through it unless they swim; the mob
    /// pathfinder treats it as crossable footing and the kinematics float mobs up out
    /// of it.
    #[inline]
    pub fn is_water(self) -> bool {
        self == Block::Water
    }

    /// This block's behaviour — the world-reactive "class" assigned in its data
    /// row (random ticks, …). Most blocks are [`behavior::INERT`].
    #[inline]
    pub fn behavior(self) -> &'static dyn BlockBehavior {
        self.def().behavior
    }

    /// What secondary-use does for this block, if anything. Interactability lives
    /// on the block row so gameplay code does not need to know which concrete block
    /// ids open menus or toggle doors.
    #[inline]
    pub fn interaction(self) -> BlockInteraction {
        self.def().interaction
    }

    /// Whether this block receives random ticks — a shortcut for
    /// `self.behavior().has_random_tick()`, read by the per-section random-tick gate
    /// and the dispatch in `world::tick`.
    #[inline]
    pub fn has_random_tick(self) -> bool {
        self.behavior().has_random_tick()
    }

    #[inline]
    pub fn is_opaque(self) -> bool {
        data::flags(self.id()).is_opaque()
    }

    /// Shape-class test the mesher runs per lighting-ring cell; the dense flag
    /// table answers it without a `def()` big-table read. Loader-derived from
    /// `shape == slab`, so it cannot disagree with [`render_shape`](Self::render_shape).
    #[inline]
    pub fn is_slab(self) -> bool {
        data::flags(self.id()).is_slab()
    }

    /// Does this block cast ambient occlusion? Full opaque cubes always do, and
    /// leaves also occlude — onto adjacent leaves and within a canopy — so dense
    /// foliage gets internal AO depth instead of reading flat. Unlike `is_opaque`,
    /// this does NOT affect face culling or skylight (leaves still draw every face
    /// and still pass light through at half attenuation). Water never occludes.
    #[inline]
    pub fn occludes_ao(self) -> bool {
        data::flags(self.id()).occludes_ao()
    }

    #[inline]
    pub fn is_transparent(self) -> bool {
        data::flags(self.id()).is_transparent()
    }

    /// Block-light this block radiates when ACTIVE, on the SAME x2 integer scale the
    /// skylight flood-fill uses (`SKY_FULL` = 30 = full daylight = level 15). `0` for
    /// non-emitters. A torch is always active at level 14 (`28` on the x2 scale):
    /// bright enough to light a cave, but one notch under open daylight so a lit cell
    /// still reads as "indoors" and takes the warm block-light tint. A furnace shares
    /// that level but only while it is LIT — that state lives in its block-entity, not
    /// the block id, so the flood seeds furnaces only in their lit state.
    #[inline]
    pub fn light_emission(self) -> u8 {
        self.def().emission
    }

    /// Optional visual-only particle emitter rows declared on this block's data
    /// row (a `particle_emitters.json` bundle reference or one inline row).
    /// Content packs add these through `blocks.json`; the client presentation
    /// layer turns loaded cells into transient render particles.
    #[inline]
    pub fn particle_emitter(self) -> Option<&'static [ParticleEmitter]> {
        self.def().particle_emitter
    }

    /// A cell a placement may overwrite in place: empty air, water (building into
    /// water displaces it), or walk-through grassy foliage — the
    /// [`Replaceable`](BlockTag::REPLACEABLE) set. Mirrors the place-gate in
    /// `game::try_place`.
    #[inline]
    pub fn is_replaceable(self) -> bool {
        self.has_tag(BlockTag::REPLACEABLE)
    }

    /// Whether this block is [`Fragile`](BlockTag::FRAGILE) — it shatters when it
    /// loses support or water enters its cell. Read by the water sim (a fragile cell
    /// is one water may flow into) and paired with the [`FRAGILE`](behavior) break
    /// behaviour on every fragile block's row.
    #[inline]
    pub fn is_fragile(self) -> bool {
        self.has_tag(BlockTag::FRAGILE)
    }

    /// Whether `ground` (the block directly below) is a surface this block may be PLACED
    /// on. Almost everything has no substrate rule and accepts anything; the plants gate
    /// by their `RootsIn*` tags, which COMBINE — a block accepts a ground if *any* of its
    /// requirements is met: [`RootsInSoil`](BlockTag::ROOTS_IN_SOIL) → [`Soil`](BlockTag::SOIL)
    /// (grass/dirt), [`RootsInSand`](BlockTag::ROOTS_IN_SAND) → [`Sand`](BlockTag::SAND)
    /// (sand/red sand), [`RootsInStone`](BlockTag::ROOTS_IN_STONE) → any
    /// [`BlockMaterial::Stone`] block. So a flower roots in soil, a cactus in sand, and a
    /// mushroom (which carries both soil + stone) in soil or stone. `game::try_place`
    /// refuses a spot this rejects. PLACEMENT only — whether an already-placed block
    /// *stays* (its support wasn't dug out) is the separate physical
    /// [`FRAGILE`](behavior::FRAGILE) check, which asks merely whether something solid is
    /// still beneath it, not what type. A block joins a substrate class by editing the
    /// `RootsIn*` tags on its data row.
    pub fn can_root_on(self, ground: Block) -> bool {
        let soil = self.has_tag(BlockTag::ROOTS_IN_SOIL);
        let sand = self.has_tag(BlockTag::ROOTS_IN_SAND);
        let stone = self.has_tag(BlockTag::ROOTS_IN_STONE);
        if !(soil || sand || stone) {
            return true; // no substrate rule — stands on anything
        }
        (soil && ground.has_tag(BlockTag::SOIL))
            || (sand && ground.has_tag(BlockTag::SAND))
            || (stone && ground.material() == BlockMaterial::Stone)
    }

    /// Whether a placed directional block should rotate its authored front toward the
    /// player. Used by bbmodel blocks the same way furnaces/chests store a placement
    /// facing for their front face.
    #[inline]
    pub fn directional_view(self) -> bool {
        data::flags(self.id()).is_directional_view()
    }

    /// Per-face tile: [top, bottom, side].
    #[inline]
    pub fn tiles(self) -> [Tile; 3] {
        self.def().tiles
    }

    /// Mining material class (drives tool requirement + future tool tiers). An
    /// internal grouping key — `pub(crate)`; the public surface is
    /// [`requires_tool`](Self::requires_tool) / [`harvest_tier`](Self::harvest_tier).
    #[inline]
    pub(crate) fn material(self) -> BlockMaterial {
        self.def().material
    }

    /// Base break-time scalar in "hardness units". `0.0` = instant; `< 0.0` =
    /// unbreakable (never a mining target). See `crate::mining` for the model.
    #[inline]
    pub fn hardness(self) -> f32 {
        self.def().hardness
    }

    /// What this block yields when harvested. `DropSpec::NONE` = no drop.
    #[inline]
    pub fn drop_spec(self) -> DropSpec {
        self.def().drop
    }

    /// The inventory item that represents this block (`Air -> Air`).
    #[inline]
    pub fn to_item(self) -> ItemType {
        ItemType::from_block(self)
    }

    /// Whether this block cannot be hand-harvested (Stone/Ore yield nothing
    /// without a pickaxe). It still breaks — it just drops nothing. Equivalent to
    /// `harvest_tier() >= 1`; mirrors the harvest gate in `crate::mining`.
    #[inline]
    pub fn requires_tool(self) -> bool {
        matches!(self.material(), BlockMaterial::Stone | BlockMaterial::Ore)
    }

    /// The tool kind that mines this block efficiently — a [`Pickaxe`](ToolKind::Pickaxe)
    /// for stone & ore, an [`Axe`](ToolKind::Axe) for wood (logs, planks, the
    /// crafting table, the chest), a [`Shovel`](ToolKind::Shovel) for dirt & sand
    /// (grass, podzol, gravel, clay, snow…) — or `None` for blocks a bare hand mines
    /// just as fast (plants, glass-likes). Holding the matching tool grants the
    /// tier speed-up in [`crate::mining::break_time`], and for tool-gated blocks the
    /// pickaxe also unlocks the drop (see [`harvest_tier`](Self::harvest_tier)); the
    /// item half of the pairing is [`ItemType::tool`](crate::item::ItemType::tool).
    #[inline]
    pub fn preferred_tool(self) -> Option<ToolKind> {
        match self.material() {
            BlockMaterial::Stone | BlockMaterial::Ore => Some(ToolKind::Pickaxe),
            BlockMaterial::Wood => Some(ToolKind::Axe),
            BlockMaterial::Dirt | BlockMaterial::Sand => Some(ToolKind::Shovel),
            _ => None,
        }
    }

    /// Minimum pickaxe tier (`0` = hand, `1` = wooden, `2` = stone, `3` = above
    /// stone) needed to HARVEST this block — i.e. to get a drop AND to mine it
    /// faster than by hand. A pickaxe below this tier breaks the block at the
    /// bare-hand rate and yields nothing (matching the goal's diamond-by-hand
    /// rule). Everything that is hand-harvestable (dirt, wood, plants, planks…)
    /// is tier `0`. Per-row in [`BlockDef`](definition::BlockDef): stone/ore
    /// blocks are tier `1`, iron/copper ore `2`, gold/diamond ore `3`.
    #[inline]
    pub fn harvest_tier(self) -> u8 {
        self.def().harvest_tier
    }

    /// The [`Sound`](crate::audio::Sound) this block makes for `action` — mining,
    /// breaking, placing, a footstep — or `None` if that interaction is silent.
    ///
    /// Data-driven and resolved by **material** (wood sounds woody, stone stony),
    /// exactly as [`preferred_tool`](Self::preferred_tool) is, so a new block of an
    /// existing material is heard automatically. The mapping lives in
    /// [`sounds`]; the simulation emits the resolved id as an `audio::SoundEvent`
    /// for the client to play (see [`crate::audio`]).
    #[inline]
    pub fn sound(self, action: BlockSoundAction) -> Option<Sound> {
        self.sound_set().get(action)
    }

    /// The shared [`BlockSoundSet`](sounds::BlockSoundSet) for this block's material.
    #[inline]
    fn sound_set(self) -> &'static sounds::BlockSoundSet {
        match self.material() {
            BlockMaterial::Wood => &sounds::WOOD,
            BlockMaterial::Stone | BlockMaterial::Ore => &sounds::STONE,
            BlockMaterial::Dirt => &sounds::DIRT,
            _ => &sounds::SILENT,
        }
    }

    #[inline]
    fn def(self) -> &'static definition::BlockDef {
        data::def(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{Block, BlockInteraction, BlockMaterial, RenderShape};
    use crate::item::ItemType;

    #[test]
    fn directional_view_is_block_data_for_blocks_with_a_front() {
        for block in [Block::Furnace, Block::Chest, Block::FurnitureWorkbench] {
            assert!(
                block.directional_view(),
                "{block:?} should face the player on placement"
            );
        }
        for block in [Block::CraftingTable, Block::Torch, Block::Stone] {
            assert!(
                !block.directional_view(),
                "{block:?} has no authored front view"
            );
        }
    }

    #[test]
    fn door_shaped_blocks_advertise_toggle_interaction() {
        let mut checked_any = false;
        for &block in Block::all() {
            if block.render_shape() != RenderShape::Door {
                continue;
            }
            checked_any = true;
            assert_eq!(
                block.interaction(),
                BlockInteraction::ToggleDoor,
                "{block:?}"
            );
        }
        assert!(checked_any, "expected at least one door block");
    }

    #[test]
    fn every_block_has_consistent_metadata() {
        for &block in Block::all() {
            let spec = block.drop_spec();
            // Every dropped item is a real (non-Air) item with a sane count range.
            for d in spec.drops {
                assert_ne!(d.item, ItemType::Air, "{block:?} drops Air");
                assert!(
                    d.min >= 1 && d.min <= d.max,
                    "{block:?} bad drop count {}..{}",
                    d.min,
                    d.max
                );
            }
            // requires_tool() is exactly the Stone/Ore material set, and matches
            // "needs at least a wooden pickaxe" (harvest_tier >= 1).
            let by_material = matches!(block.material(), BlockMaterial::Stone | BlockMaterial::Ore);
            assert_eq!(block.requires_tool(), by_material, "{block:?}");
            assert_eq!(
                block.requires_tool(),
                block.harvest_tier() >= 1,
                "{block:?}"
            );
        }
    }

    #[test]
    fn preferred_tool_pairs_pickaxe_axe_shovel_with_their_materials() {
        use crate::item::ToolKind;
        // Stone & ore want a pickaxe.
        for b in [
            Block::Stone,
            Block::Cobblestone,
            Block::CoalOre,
            Block::DiamondOre,
        ] {
            assert_eq!(b.preferred_tool(), Some(ToolKind::Pickaxe), "{b:?}");
        }
        // Wood wants an axe — logs and planks, AND (sanity check) the crafting
        // table and chest, which are Wood-material blocks.
        for b in [
            Block::OakLog,
            Block::OakPlanks,
            Block::CraftingTable,
            Block::Chest,
        ] {
            assert_eq!(b.material(), BlockMaterial::Wood, "{b:?} should be wood");
            assert_eq!(b.preferred_tool(), Some(ToolKind::Axe), "{b:?}");
        }
        // Dirt & sand want a shovel — the soft cover blocks (grass, podzol, gravel,
        // clay, snow), all hand-harvestable so the shovel is a pure speed bonus.
        for b in [
            Block::Dirt,
            Block::Grass,
            Block::Podzol,
            Block::Sand,
            Block::Gravel,
            Block::Clay,
            Block::Snow,
        ] {
            assert!(
                matches!(b.material(), BlockMaterial::Dirt | BlockMaterial::Sand),
                "{b:?} should be dirt/sand"
            );
            assert_eq!(b.preferred_tool(), Some(ToolKind::Shovel), "{b:?}");
        }
        // Everything a hand mines just as well has no preferred tool (plants,
        // leaves, air).
        for b in [
            Block::Poppy,
            Block::ShortGrass,
            Block::OakLeaves,
            Block::Air,
        ] {
            assert_eq!(b.preferred_tool(), None, "{b:?}");
        }
    }

    #[test]
    fn is_terrain_solid_is_the_bare_ground_set() {
        // Exactly the natural ground blocks — the set the genmap audits treat as
        // terrain (excludes logs/leaves and built blocks).
        let terrain = [
            Block::Stone,
            Block::Dirt,
            Block::Grass,
            Block::Sand,
            Block::Snow,
        ];
        for &b in &terrain {
            assert!(b.is_terrain_solid(), "{b:?} should be terrain-solid");
        }
        for &b in Block::all() {
            let expected = terrain.contains(&b);
            assert_eq!(b.is_terrain_solid(), expected, "{b:?}");
        }
        // Notably NOT terrain even though solid: tree parts and built blocks.
        for b in [
            Block::OakLog,
            Block::OakLeaves,
            Block::Cobblestone,
            Block::Sandstone,
            Block::Water,
            Block::Air,
        ] {
            assert!(!b.is_terrain_solid(), "{b:?} should NOT be terrain-solid");
        }
    }
}
