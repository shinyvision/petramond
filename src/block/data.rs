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
    "petramond:air",
    "petramond:grass",
    "petramond:dirt",
    "petramond:stone",
    "petramond:sand",
    "petramond:snow",
    "petramond:water",
    "petramond:oak_log",
    "petramond:oak_leaves",
    "petramond:spruce_log",
    "petramond:birch_log",
    "petramond:jungle_log",
    "petramond:acacia_log",
    "petramond:spruce_leaves",
    "petramond:birch_leaves",
    "petramond:jungle_leaves",
    "petramond:acacia_leaves",
    "petramond:azalea_leaves",
    "petramond:red_sand",
    "petramond:sandstone",
    "petramond:red_sandstone",
    "petramond:terracotta",
    "petramond:white_terracotta",
    "petramond:orange_terracotta",
    "petramond:yellow_terracotta",
    "petramond:brown_terracotta",
    "petramond:red_terracotta",
    "petramond:light_gray_terracotta",
    "petramond:podzol",
    "petramond:mycelium",
    "petramond:coarse_dirt",
    "petramond:gravel",
    "petramond:clay",
    "petramond:mud",
    "petramond:moss_block",
    "petramond:snow_block",
    "petramond:packed_ice",
    "petramond:ice",
    "petramond:calcite",
    "petramond:marble",
    "petramond:tuff",
    "petramond:coal_ore",
    "petramond:iron_ore",
    "petramond:copper_ore",
    "petramond:gold_ore",
    "petramond:diamond_ore",
    "petramond:pumpkin",
    "petramond:melon",
    "petramond:cactus",
    "petramond:short_grass",
    "petramond:fern",
    "petramond:dandelion",
    "petramond:poppy",
    "petramond:cornflower",
    "petramond:allium",
    "petramond:azure_bluet",
    "petramond:oxeye_daisy",
    "petramond:red_tulip",
    "petramond:dead_bush",
    "petramond:brown_mushroom",
    "petramond:red_mushroom",
    "petramond:cobblestone",
    "petramond:oak_planks",
    "petramond:spruce_planks",
    "petramond:birch_planks",
    "petramond:jungle_planks",
    "petramond:acacia_planks",
    "petramond:crafting_table",
    "petramond:furnace",
    "petramond:chest",
    "petramond:torch",
    "petramond:furniture_workbench",
    "petramond:oak_sapling",
    "petramond:spruce_sapling",
    "petramond:birch_sapling",
    "petramond:jungle_sapling",
    "petramond:acacia_sapling",
    "petramond:oak_door",
    "petramond:spruce_door",
    "petramond:birch_door",
    "petramond:jungle_door",
    "petramond:acacia_door",
    "petramond:redwood_log",
    "petramond:redwood_leaves",
    "petramond:redwood_planks",
    "petramond:redwood_door",
    "petramond:oak_stairs",
    "petramond:spruce_stairs",
    "petramond:birch_stairs",
    "petramond:jungle_stairs",
    "petramond:acacia_stairs",
    "petramond:redwood_stairs",
    "petramond:cobblestone_stairs",
    "petramond:stone_stairs",
    "petramond:dirt_stairs",
    "petramond:bed_frame",
    "petramond:bed",
    "petramond:oak_slab",
    "petramond:spruce_slab",
    "petramond:birch_slab",
    "petramond:jungle_slab",
    "petramond:acacia_slab",
    "petramond:redwood_slab",
    "petramond:cobblestone_slab",
    "petramond:stone_slab",
    "petramond:dirt_slab",
    "petramond:glass",
    "petramond:glass_pane",
    "petramond:wool_block",
    "petramond:wool_stairs",
    "petramond:wool_slab",
    "petramond:polished_marble",
    "petramond:marble_stairs",
    "petramond:marble_slab",
    "petramond:polished_marble_stairs",
    "petramond:polished_marble_slab",
    "petramond:ladder",
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
