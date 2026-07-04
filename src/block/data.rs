//! Block table plumbing: the engine name list + the JSON-loaded registry.
//!
//! The rows themselves live in `assets/blocks.json` (see `super::load`), so
//! block properties are editable — and moddable — without a rebuild. This
//! module keeps only what must stay compiled in: the engine block NAMES in
//! frozen id order (index == id — the completeness oracle the loader validates
//! the file against, and the low half of the runtime name table packs extend;
//! see `crate::registry`) and the lazily-loaded registry the `Block`
//! accessors read.

use std::sync::LazyLock;

use super::definition::{BlockDef, BlockFlags};
use super::{load, Block};

/// Engine block names in frozen id order (`ENGINE_BLOCK_NAMES[id]` names
/// `Block(id)`). Append-only: worldgen output and save palettes identify
/// blocks by these ids/names. Must stay in lockstep with the consts on
/// [`Block`]; the shipped `blocks.json` covering every name (a startup gate
/// and a test) keeps a typo here from going unnoticed.
pub(crate) const ENGINE_BLOCK_NAMES: &[&str] = &[
    "llama:air",
    "llama:grass",
    "llama:dirt",
    "llama:stone",
    "llama:sand",
    "llama:snow",
    "llama:water",
    "llama:oak_log",
    "llama:oak_leaves",
    "llama:spruce_log",
    "llama:birch_log",
    "llama:jungle_log",
    "llama:acacia_log",
    "llama:dark_oak_log",
    "llama:cherry_log",
    "llama:mangrove_log",
    "llama:spruce_leaves",
    "llama:birch_leaves",
    "llama:jungle_leaves",
    "llama:acacia_leaves",
    "llama:dark_oak_leaves",
    "llama:mangrove_leaves",
    "llama:cherry_leaves",
    "llama:azalea_leaves",
    "llama:red_sand",
    "llama:sandstone",
    "llama:red_sandstone",
    "llama:terracotta",
    "llama:white_terracotta",
    "llama:orange_terracotta",
    "llama:yellow_terracotta",
    "llama:brown_terracotta",
    "llama:red_terracotta",
    "llama:light_gray_terracotta",
    "llama:podzol",
    "llama:mycelium",
    "llama:coarse_dirt",
    "llama:gravel",
    "llama:clay",
    "llama:mud",
    "llama:moss_block",
    "llama:snow_block",
    "llama:packed_ice",
    "llama:ice",
    "llama:calcite",
    "llama:granite",
    "llama:diorite",
    "llama:andesite",
    "llama:tuff",
    "llama:coal_ore",
    "llama:iron_ore",
    "llama:copper_ore",
    "llama:gold_ore",
    "llama:redstone_ore",
    "llama:lapis_ore",
    "llama:diamond_ore",
    "llama:emerald_ore",
    "llama:pumpkin",
    "llama:melon",
    "llama:cactus",
    "llama:short_grass",
    "llama:fern",
    "llama:dandelion",
    "llama:poppy",
    "llama:cornflower",
    "llama:allium",
    "llama:azure_bluet",
    "llama:oxeye_daisy",
    "llama:red_tulip",
    "llama:dead_bush",
    "llama:brown_mushroom",
    "llama:red_mushroom",
    "llama:cobblestone",
    "llama:oak_planks",
    "llama:spruce_planks",
    "llama:birch_planks",
    "llama:jungle_planks",
    "llama:acacia_planks",
    "llama:dark_oak_planks",
    "llama:cherry_planks",
    "llama:mangrove_planks",
    "llama:crafting_table",
    "llama:furnace",
    "llama:chest",
    "llama:torch",
    "llama:furniture_workbench",
    "llama:oak_sapling",
    "llama:spruce_sapling",
    "llama:birch_sapling",
    "llama:jungle_sapling",
    "llama:acacia_sapling",
    "llama:dark_oak_sapling",
    "llama:cherry_sapling",
    "llama:oak_door",
    "llama:spruce_door",
    "llama:birch_door",
    "llama:jungle_door",
    "llama:acacia_door",
    "llama:dark_oak_door",
    "llama:cherry_door",
    "llama:mangrove_door",
    "llama:redwood_log",
    "llama:redwood_leaves",
    "llama:redwood_planks",
    "llama:redwood_door",
    "llama:oak_stairs",
    "llama:spruce_stairs",
    "llama:birch_stairs",
    "llama:jungle_stairs",
    "llama:acacia_stairs",
    "llama:dark_oak_stairs",
    "llama:cherry_stairs",
    "llama:mangrove_stairs",
    "llama:redwood_stairs",
    "llama:cobblestone_stairs",
    "llama:stone_stairs",
    "llama:dirt_stairs",
    "llama:bed_frame",
    "llama:bed",
];

/// The JSON-loaded block table. Loads exactly once, on first access from any
/// thread (the gen/light worker pools included); the loader panics with a
/// precise message if the file is missing or inconsistent (see `super::load`).
static REGISTRY: LazyLock<load::Registry> = LazyLock::new(load::registry);

/// Every registered block in id order (engine + pack-registered).
pub(super) fn all() -> &'static [Block] {
    static ALL: LazyLock<Vec<Block>> =
        LazyLock::new(|| (0..REGISTRY.defs.len()).map(|id| Block(id as u8)).collect());
    &ALL
}

#[inline]
pub(super) fn from_id(id: u8) -> Block {
    REGISTRY
        .defs
        .get(id as usize)
        .map_or(Block::Air, |d| d.block)
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
