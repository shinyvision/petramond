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
    "air",
    "grass",
    "dirt",
    "stone",
    "sand",
    "snow",
    "water",
    "oak_log",
    "oak_leaves",
    "spruce_log",
    "birch_log",
    "jungle_log",
    "acacia_log",
    "dark_oak_log",
    "cherry_log",
    "mangrove_log",
    "spruce_leaves",
    "birch_leaves",
    "jungle_leaves",
    "acacia_leaves",
    "dark_oak_leaves",
    "mangrove_leaves",
    "cherry_leaves",
    "azalea_leaves",
    "red_sand",
    "sandstone",
    "red_sandstone",
    "terracotta",
    "white_terracotta",
    "orange_terracotta",
    "yellow_terracotta",
    "brown_terracotta",
    "red_terracotta",
    "light_gray_terracotta",
    "podzol",
    "mycelium",
    "coarse_dirt",
    "gravel",
    "clay",
    "mud",
    "moss_block",
    "snow_block",
    "packed_ice",
    "ice",
    "calcite",
    "granite",
    "diorite",
    "andesite",
    "tuff",
    "coal_ore",
    "iron_ore",
    "copper_ore",
    "gold_ore",
    "redstone_ore",
    "lapis_ore",
    "diamond_ore",
    "emerald_ore",
    "pumpkin",
    "melon",
    "cactus",
    "short_grass",
    "fern",
    "dandelion",
    "poppy",
    "cornflower",
    "allium",
    "azure_bluet",
    "oxeye_daisy",
    "red_tulip",
    "dead_bush",
    "brown_mushroom",
    "red_mushroom",
    "cobblestone",
    "oak_planks",
    "spruce_planks",
    "birch_planks",
    "jungle_planks",
    "acacia_planks",
    "dark_oak_planks",
    "cherry_planks",
    "mangrove_planks",
    "crafting_table",
    "furnace",
    "chest",
    "torch",
    "furniture_workbench",
    "oak_sapling",
    "spruce_sapling",
    "birch_sapling",
    "jungle_sapling",
    "acacia_sapling",
    "dark_oak_sapling",
    "cherry_sapling",
    "oak_door",
    "spruce_door",
    "birch_door",
    "jungle_door",
    "acacia_door",
    "dark_oak_door",
    "cherry_door",
    "mangrove_door",
    "redwood_log",
    "redwood_leaves",
    "redwood_planks",
    "redwood_door",
    "oak_stairs",
    "spruce_stairs",
    "birch_stairs",
    "jungle_stairs",
    "acacia_stairs",
    "dark_oak_stairs",
    "cherry_stairs",
    "mangrove_stairs",
    "redwood_stairs",
    "cobblestone_stairs",
    "stone_stairs",
    "dirt_stairs",
    "bed_frame",
    "bed",
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
