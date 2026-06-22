//! Block registry + per-face tile mapping.

use crate::atlas::Tile;
use crate::item::{DropSpec, ItemType};

mod data;
mod definition;

pub use definition::BlockMaterial;

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
}

/// How a block's geometry is meshed. `Cube` is the standard 6-face box; `Cross`
/// is an X of two diagonal billboard quads (grass, ferns, flowers, mushrooms).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderShape {
    Cube,
    Cross,
}

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
            _ => RenderShape::Cube,
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

    /// Mining material class (drives tool requirement + future tool tiers).
    #[inline]
    pub fn material(self) -> BlockMaterial {
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

    /// Minimum pickaxe tier (`0` = hand, `1` = wooden, `2` = stone) needed to
    /// HARVEST this block — i.e. to get a drop AND to mine it faster than by hand.
    /// A pickaxe below this tier breaks the block at the bare-hand rate and yields
    /// nothing (matching the goal's redstone/diamond rule). Everything that is
    /// hand-harvestable (dirt, wood, plants, planks…) is tier `0`.
    ///
    /// A `match`, not a `BlockDef` field, so the cube rows stay untouched — only
    /// the ores that deviate from "Stone/Ore ⇒ wooden" are listed (mirrors how
    /// `render_shape` lists only the plants).
    #[inline]
    pub fn harvest_tier(self) -> u8 {
        use Block::*;
        match self {
            // Above stone tier: needs an iron pickaxe (which doesn't exist yet),
            // so a wooden/stone pickaxe breaks them at hand speed for no drop.
            GoldOre | RedstoneOre | LapisOre | DiamondOre | EmeraldOre => 3,
            // Stone pickaxe ores.
            IronOre | CopperOre => 2,
            // Everything else: wooden pickaxe for any stone/ore, hand otherwise.
            _ => match self.material() {
                BlockMaterial::Stone | BlockMaterial::Ore => 1,
                _ => 0,
            },
        }
    }

    #[inline]
    fn def(self) -> &'static definition::BlockDef {
        data::def(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{data, Block, BlockMaterial};
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
        ];

        assert_eq!(Block::ALL, expected);
        for (id, block) in expected.into_iter().enumerate() {
            assert_eq!(block.id(), id as u8);
            assert_eq!(Block::from_id(id as u8), block);
        }
        assert_eq!(Block::from_id(u8::MAX), Block::Air);
    }

    #[test]
    fn definitions_are_id_ordered() {
        assert_eq!(data::BLOCK_DEFS.len(), Block::ALL.len());
        for def in data::BLOCK_DEFS {
            assert_eq!(Block::from_id(def.block.id()), def.block);
            assert_eq!(data::BLOCK_DEFS[def.block.id() as usize].block, def.block);
        }
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
        assert_eq!(Block::IronOre.drop_spec().drops, &[drop1(ItemType::RawIron)]);
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
            assert_eq!(block.requires_tool(), block.harvest_tier() >= 1, "{block:?}");
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
}
