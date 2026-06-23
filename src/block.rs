//! Block registry + per-face tile mapping.

use crate::atlas::Tile;
use crate::item::{DropSpec, ItemType};

pub mod behavior;
mod data;
mod definition;

pub use behavior::BlockBehavior;
pub(crate) use definition::BlockMaterial;

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
    // --- Crafting update: crafted / placeable blocks (ids 70..). ---
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
    // --- Furnace update (id 82). ---
    Furnace,
    // --- Chest update (id 83). ---
    Chest,
    // --- Torch update (id 84). ---
    Torch,
}

/// A named category a block belongs to — a PROPERTY OF BLOCKS, exactly as
/// [`ItemTag`](crate::item::ItemTag) is a property of items. Each block lists its
/// tags in its [`BlockDef`](definition::BlockDef) data row and code asks via
/// [`Block::has_tag`]; keeping membership in the data means a block joins a
/// category by editing its row, never a `match` in this file. Tags answer "what
/// *is* this block" (categorisation); [`behavior`] answers "what does it *do*".
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockTag {
    /// Any tree-leaves block: takes random ticks and decays when cut off, and
    /// counts as the support that keeps an adjacent leaf alive.
    Leaves,
    /// Any tree-log block: counts as support that keeps adjacent leaves alive.
    Log,
    /// Natural ground surface — the bare-terrain set (stone/dirt/grass/sand/snow),
    /// excluding tree parts and built blocks. Worldgen audits measure overhangs /
    /// floating debris against it (see `worldgen::audit`).
    Terrain,
}

/// How a block's geometry is meshed. `Cube` is the standard 6-face box; `Cross`
/// is an X of two diagonal billboard quads (grass, ferns, flowers, mushrooms);
/// `Torch` is a thin pole (a small box) standing on the floor or tilted against a
/// wall, with its orientation read from the chunk's torch map (see `mesh::torch`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderShape {
    Cube,
    Cross,
    Torch,
}

/// One axis-aligned box of a block's collision shape, in CELL-LOCAL coordinates
/// (`0.0..1.0` per axis). A block's full shape is a *list* of these (see
/// [`Block::collision_boxes`]) — one for a full cube or the inset chest, several for
/// shapes like stairs. The player collides via a swept-AABB over them, and the
/// selection outline + break overlay derive from their union.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// The whole cell — the collision shape of every ordinary solid block.
const FULL_CUBE_BOXES: &[Aabb] = &[Aabb {
    min: [0.0, 0.0, 0.0],
    max: [1.0, 1.0, 1.0],
}];
/// The chest's inset body+lid box (1/16 inset, 14/16 tall) — matches the model in
/// `render::chest_model`, so collision, outline, and crack all hug the chest.
const CHEST_BOXES: &[Aabb] = &[Aabb {
    min: [1.0 / 16.0, 0.0, 1.0 / 16.0],
    max: [15.0 / 16.0, 14.0 / 16.0, 15.0 / 16.0],
}];

impl Block {
    pub const ALL: &'static [Block] = data::ALL_BLOCKS;

    /// Mesh geometry kind. Cross-model plants render as billboards; everything
    /// else is a full cube. (A match, not a `BlockDef` field, so the 60 cube rows
    /// stay untouched — only the handful of plants are listed here.)
    #[inline]
    pub fn render_shape(self) -> RenderShape {
        use Block::*;
        match self {
            ShortGrass | Fern | Dandelion | Poppy | Cornflower | Allium | AzureBluet
            | OxeyeDaisy | RedTulip | DeadBush | BrownMushroom | RedMushroom => RenderShape::Cross,
            Torch => RenderShape::Torch,
            _ => RenderShape::Cube,
        }
    }

    /// The block's collision shape: a list of cell-local AABBs (`0.0..1.0`). Empty =
    /// no collision (air, water, walk-through plants). One unit box for an ordinary
    /// full cube; the chest is a single inset box; future stairs/slabs return several.
    /// The single source of truth for player collision AND — via the union — the
    /// selection outline + break overlay ([`visual_aabb`](Self::visual_aabb)).
    #[inline]
    pub fn collision_boxes(self) -> &'static [Aabb] {
        if !self.is_solid() {
            return &[];
        }
        match self {
            Block::Chest => CHEST_BOXES,
            // A torch is SOLID so the ray can select it, but has NO collision box —
            // the player walks through it. Its selection outline is custom-shaped
            // (see `player::interaction`), not derived from this empty list.
            Block::Torch => &[],
            _ => FULL_CUBE_BOXES,
        }
    }

    /// Whether this block physically obstructs movement — i.e. has any collision
    /// box. The single predicate for "can an entity rest on / be stopped by this
    /// cell", derived from [`collision_boxes`](Self::collision_boxes) (the source of
    /// truth) rather than [`is_solid`](Self::is_solid): a torch is solid (so the ray
    /// can select it) yet has NO collision, so items and particles fall through it.
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
        self.has_tag(BlockTag::Terrain)
    }

    /// Whether this is any tree-leaves variant. Leaves form the canopy: they take
    /// random ticks and decay when cut off from wood, and are the support a
    /// neighbouring leaf looks for (alongside logs). See [`behavior`].
    #[inline]
    pub fn is_leaves(self) -> bool {
        self.has_tag(BlockTag::Leaves)
    }

    /// Whether this is any tree-log variant. A log keeps nearby leaves alive: a
    /// leaf with no log within a few steps (through leaves) decays — see the flood
    /// in [`behavior`].
    #[inline]
    pub fn is_log(self) -> bool {
        self.has_tag(BlockTag::Log)
    }

    /// This block's behaviour — the world-reactive "class" assigned in its data
    /// row (random ticks, …). Most blocks are [`behavior::INERT`].
    #[inline]
    pub fn behavior(self) -> &'static dyn BlockBehavior {
        self.def().behavior
    }

    /// Whether this block receives random ticks — a shortcut for
    /// `self.behavior().has_random_tick()`, read by the per-chunk random-tick gate
    /// and the dispatch in `world::tick`.
    #[inline]
    pub fn has_random_tick(self) -> bool {
        self.behavior().has_random_tick()
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

    /// Block-light this block radiates when ACTIVE, on the SAME x2 integer scale the
    /// skylight flood-fill uses (`SKY_FULL` = 30 = full daylight = level 15). `0` for
    /// non-emitters. A torch is always active at level 14 (`28` on the x2 scale):
    /// bright enough to light a cave, but one notch under open daylight so a lit cell
    /// still reads as "indoors" and takes the warm block-light tint. A furnace shares
    /// that level but only while it is LIT — that state lives in its block-entity, not
    /// the block id, so the flood seeds furnaces only in their lit state (see
    /// `world::light_queue` for which cells are seeded and `mesh::blocklight` for the
    /// flood + the sky-vs-block tinting).
    #[inline]
    pub fn light_emission(self) -> u8 {
        match self {
            Block::Torch | Block::Furnace => 28,
            _ => 0,
        }
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

    /// Minimum pickaxe tier (`0` = hand, `1` = wooden, `2` = stone, `3` = above
    /// stone) needed to HARVEST this block — i.e. to get a drop AND to mine it
    /// faster than by hand. A pickaxe below this tier breaks the block at the
    /// bare-hand rate and yields nothing (matching the goal's redstone/diamond
    /// rule). Everything that is hand-harvestable (dirt, wood, plants, planks…)
    /// is tier `0`. Per-row in [`BlockDef`](definition::BlockDef): stone/ore
    /// blocks are tier `1`, iron/copper ore `2`, gold/redstone/lapis/diamond/
    /// emerald ore `3`.
    #[inline]
    pub fn harvest_tier(self) -> u8 {
        self.def().harvest_tier
    }

    #[inline]
    fn def(self) -> &'static definition::BlockDef {
        data::def(self)
    }
}

/// One-line delegating call for the shared id-ordering test in [`crate::registry`]:
/// the `BLOCK_DEFS` table is id-ordered and one-to-one with [`Block::ALL`].
#[cfg(test)]
pub(crate) fn assert_registry_ordered() {
    crate::registry::assert_id_ordered(data::BLOCK_DEFS, Block::ALL);
}

#[cfg(test)]
mod tests {
    use super::{Block, BlockMaterial};
    use crate::atlas::Tile;
    use crate::item::{Drop, ItemType};

    /// One exact drop: `count` of `item`. Test shorthand for the `Drop` literal.
    fn drop1(item: ItemType) -> Drop {
        Drop {
            item,
            min: 1,
            max: 1,
        }
    }

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
            Block::SpruceLog,
            Block::BirchLog,
            Block::JungleLog,
            Block::AcaciaLog,
            Block::DarkOakLog,
            Block::CherryLog,
            Block::MangroveLog,
            Block::SpruceLeaves,
            Block::BirchLeaves,
            Block::JungleLeaves,
            Block::AcaciaLeaves,
            Block::DarkOakLeaves,
            Block::MangroveLeaves,
            Block::CherryLeaves,
            Block::AzaleaLeaves,
            Block::RedSand,
            Block::Sandstone,
            Block::RedSandstone,
            Block::Terracotta,
            Block::WhiteTerracotta,
            Block::OrangeTerracotta,
            Block::YellowTerracotta,
            Block::BrownTerracotta,
            Block::RedTerracotta,
            Block::LightGrayTerracotta,
            Block::Podzol,
            Block::Mycelium,
            Block::CoarseDirt,
            Block::Gravel,
            Block::Clay,
            Block::Mud,
            Block::MossBlock,
            Block::SnowBlock,
            Block::PackedIce,
            Block::Ice,
            Block::Calcite,
            Block::Granite,
            Block::Diorite,
            Block::Andesite,
            Block::Tuff,
            Block::CoalOre,
            Block::IronOre,
            Block::CopperOre,
            Block::GoldOre,
            Block::RedstoneOre,
            Block::LapisOre,
            Block::DiamondOre,
            Block::EmeraldOre,
            Block::Pumpkin,
            Block::Melon,
            Block::Cactus,
            Block::ShortGrass,
            Block::Fern,
            Block::Dandelion,
            Block::Poppy,
            Block::Cornflower,
            Block::Allium,
            Block::AzureBluet,
            Block::OxeyeDaisy,
            Block::RedTulip,
            Block::DeadBush,
            Block::BrownMushroom,
            Block::RedMushroom,
            Block::Cobblestone,
            Block::OakPlanks,
            Block::SprucePlanks,
            Block::BirchPlanks,
            Block::JunglePlanks,
            Block::AcaciaPlanks,
            Block::DarkOakPlanks,
            Block::CherryPlanks,
            Block::MangrovePlanks,
            Block::CraftingTable,
            Block::Furnace,
            Block::Chest,
            Block::Torch,
        ];

        assert_eq!(Block::ALL, expected);
        for (id, block) in expected.into_iter().enumerate() {
            assert_eq!(block.id(), id as u8);
            assert_eq!(Block::from_id(id as u8), block);
        }
        assert_eq!(Block::from_id(u8::MAX), Block::Air);
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
    fn metadata_matches_contract() {
        // Wood: hardness 2.0 (5 s by hand), drops self, hand-harvestable.
        assert_eq!(Block::OakLog.material(), BlockMaterial::Wood);
        assert_eq!(Block::OakLog.hardness(), 2.0);
        assert!(!Block::OakLog.requires_tool());
        assert_eq!(Block::OakLog.harvest_tier(), 0);
        assert_eq!(Block::OakLog.drop_spec().drops, &[drop1(ItemType::OakLog)]);

        // Stone needs a wooden pickaxe (tier 1) and, when harvested, yields
        // cobblestone rather than itself.
        assert_eq!(Block::Stone.material(), BlockMaterial::Stone);
        assert_eq!(Block::Stone.hardness(), 1.5);
        assert!(Block::Stone.requires_tool());
        assert_eq!(Block::Stone.harvest_tier(), 1);
        assert_eq!(
            Block::Stone.drop_spec().drops,
            &[drop1(ItemType::Cobblestone)]
        );

        // Ores require a tool and are harder. Coal is wooden-tier; iron/copper
        // need a stone pickaxe; redstone/diamond sit above the stone tier.
        assert_eq!(Block::CoalOre.material(), BlockMaterial::Ore);
        assert_eq!(Block::CoalOre.hardness(), 3.0);
        assert!(Block::CoalOre.requires_tool());
        assert_eq!(Block::CoalOre.harvest_tier(), 1);
        assert_eq!(Block::CoalOre.drop_spec().drops, &[drop1(ItemType::Coal)]);
        assert_eq!(Block::IronOre.harvest_tier(), 2);
        assert_eq!(
            Block::IronOre.drop_spec().drops,
            &[drop1(ItemType::RawIron)]
        );
        assert_eq!(Block::CopperOre.harvest_tier(), 2);
        assert_eq!(
            Block::CopperOre.drop_spec().drops,
            &[Drop {
                item: ItemType::RawCopper,
                min: 2,
                max: 4,
            }]
        );
        assert_eq!(Block::DiamondOre.harvest_tier(), 3);

        // Leaves: soft foliage, drop self.
        assert_eq!(Block::OakLeaves.material(), BlockMaterial::Foliage);
        assert_eq!(Block::OakLeaves.hardness(), 0.2);
        assert!(!Block::OakLeaves.requires_tool());

        // Dirt family.
        assert_eq!(Block::Grass.material(), BlockMaterial::Dirt);
        assert_eq!(Block::Grass.hardness(), 0.5);
        assert_eq!(Block::Grass.drop_spec().drops, &[drop1(ItemType::Grass)]);

        // Cross-plants: instant, Plant material, never require a tool.
        for plant in [
            Block::Poppy,
            Block::Fern,
            Block::RedMushroom,
            Block::DeadBush,
        ] {
            assert_eq!(plant.material(), BlockMaterial::Plant, "{plant:?}");
            assert_eq!(plant.hardness(), 0.0, "{plant:?}");
            assert!(!plant.requires_tool(), "{plant:?}");
            assert_eq!(plant.drop_spec().drops.len(), 1, "{plant:?}");
        }

        // ShortGrass: instant, drops NOTHING (matches the goal's "grass does not drop").
        assert_eq!(Block::ShortGrass.material(), BlockMaterial::Plant);
        assert_eq!(Block::ShortGrass.hardness(), 0.0);
        assert!(Block::ShortGrass.drop_spec().drops.is_empty());

        // Air / Water: unbreakable, no material, no drop, never need a tool.
        for b in [Block::Air, Block::Water] {
            assert_eq!(b.material(), BlockMaterial::None, "{b:?}");
            assert_eq!(b.hardness(), -1.0, "{b:?}");
            assert!(b.drop_spec().drops.is_empty(), "{b:?}");
            assert!(!b.requires_tool(), "{b:?}");
        }

        // to_item mirrors the item conversion.
        assert_eq!(Block::Stone.to_item(), ItemType::Stone);
        assert_eq!(Block::Air.to_item(), ItemType::Air);
    }

    #[test]
    fn every_block_has_consistent_metadata() {
        for &block in Block::ALL {
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
        for &b in Block::ALL {
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

    #[test]
    fn tags_drive_category_predicates() {
        use super::BlockTag;

        // Every *Leaves variant carries the Leaves tag (and is not a log); the
        // predicate reads straight from the data row.
        for b in [
            Block::OakLeaves,
            Block::SpruceLeaves,
            Block::BirchLeaves,
            Block::JungleLeaves,
            Block::AcaciaLeaves,
            Block::DarkOakLeaves,
            Block::MangroveLeaves,
            Block::CherryLeaves,
            Block::AzaleaLeaves,
        ] {
            assert!(b.is_leaves() && b.has_tag(BlockTag::Leaves), "{b:?}");
            assert!(!b.is_log(), "{b:?}");
        }

        // Every *Log variant carries the Log tag (and is not leaves).
        for b in [
            Block::OakLog,
            Block::SpruceLog,
            Block::BirchLog,
            Block::JungleLog,
            Block::AcaciaLog,
            Block::DarkOakLog,
            Block::CherryLog,
            Block::MangroveLog,
        ] {
            assert!(b.is_log() && b.has_tag(BlockTag::Log), "{b:?}");
            assert!(!b.is_leaves(), "{b:?}");
        }

        // Non-tree blocks carry neither tree tag.
        for b in [Block::Stone, Block::OakPlanks, Block::Air, Block::Water] {
            assert!(!b.is_leaves() && !b.is_log(), "{b:?}");
        }
    }

    #[test]
    fn behavior_drives_random_tick_and_matches_leaves() {
        // The random-tick property comes from each block's behaviour, and today it
        // holds exactly for leaves — i.e. every leaf row points at a leaf-decay
        // behaviour and no other row claims a random tick. Guards the table from a
        // leaf row left on `INERT` (or a stray non-leaf marked tickable).
        for &b in Block::ALL {
            assert_eq!(b.has_random_tick(), b.is_leaves(), "{b:?}");
        }
    }
}
