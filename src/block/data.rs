//! Block table plumbing: the id-ordered variant list + the JSON-loaded registry.
//!
//! The rows themselves live in `assets/blocks.json` (see `super::load`), so
//! block properties are editable — and moddable — without a rebuild. This
//! module keeps only what must stay compiled in: the list of `Block` variants
//! in id order (the completeness oracle the loader validates the file against)
//! and the lazily-loaded registry the `Block` accessors read.

use std::sync::LazyLock;

use super::definition::{BlockDef, BlockFlags};
use super::{load, Block};

pub(super) const ALL_BLOCKS: &[Block] = &[
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
    Block::FurnitureWorkbench,
    Block::OakSapling,
    Block::SpruceSapling,
    Block::BirchSapling,
    Block::JungleSapling,
    Block::AcaciaSapling,
    Block::DarkOakSapling,
    Block::CherrySapling,
    Block::OakDoor,
    Block::SpruceDoor,
    Block::BirchDoor,
    Block::JungleDoor,
    Block::AcaciaDoor,
    Block::DarkOakDoor,
    Block::CherryDoor,
    Block::MangroveDoor,
    Block::RedwoodLog,
    Block::RedwoodLeaves,
    Block::RedwoodPlanks,
    Block::RedwoodDoor,
    Block::OakStairs,
    Block::SpruceStairs,
    Block::BirchStairs,
    Block::JungleStairs,
    Block::AcaciaStairs,
    Block::DarkOakStairs,
    Block::CherryStairs,
    Block::MangroveStairs,
    Block::RedwoodStairs,
    Block::CobblestoneStairs,
    Block::StoneStairs,
    Block::DirtStairs,
    Block::BedFrame,
    Block::Bed,
];

/// The JSON-loaded block table. Loads exactly once, on first access from any
/// thread (the gen/light worker pools included); the loader panics with a
/// precise message if the file is missing or inconsistent (see `super::load`).
static REGISTRY: LazyLock<load::Registry> = LazyLock::new(load::registry);

#[inline]
pub(super) fn from_id(id: u8) -> Block {
    REGISTRY.defs.get(id as usize).map_or(Block::Air, |d| d.block)
}

#[inline]
pub(super) fn def(block: Block) -> &'static BlockDef {
    &REGISTRY.defs[block.id() as usize]
}

/// Dense per-id copy of every block's [`BlockFlags`], indexed by raw block id.
///
/// The mesher/light hot loops test `is_opaque`/`occludes_ao` on neighbour ids tens of
/// times per emitted face. Going through [`def`] loads a pointer into the large
/// `BlockDef` array (≈100 rows × dozens of bytes, scattered across many cache lines) just
/// to read one flag byte. This table is 256 bytes — a handful of cache lines that stay hot
/// — so a flag query is one small-array read, not a big-struct indirection. It is derived
/// from the loaded defs by the loader, so it can never disagree with the source of truth.
#[inline]
pub(super) fn flags(id: u8) -> BlockFlags {
    REGISTRY.flags[id as usize]
}
