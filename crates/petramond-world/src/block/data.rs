use std::sync::OnceLock;

use super::definition::{BlockDef, BlockFlags};
use super::shape_kind::{BlockShapeKind, ShapeKindDef};
use super::{load, Block};

pub const ENGINE_BLOCK_NAMES: &[&str] = &[
    "petramond:air",
    "petramond:grass",
    "petramond:dirt",
    "petramond:stone",
    "petramond:sand",
    "petramond:snow_layer",
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
    "petramond:oak_sapling_1",
    "petramond:oak_sapling_2",
    "petramond:spruce_sapling_1",
    "petramond:spruce_sapling_2",
    "petramond:birch_sapling_1",
    "petramond:birch_sapling_2",
    "petramond:jungle_sapling_1",
    "petramond:jungle_sapling_2",
    "petramond:acacia_sapling_1",
    "petramond:acacia_sapling_2",
    "petramond:furnace_lit",
    "petramond:ladder_south",
    "petramond:ladder_west",
    "petramond:ladder_east",
    "petramond:oak_fence",
    "petramond:spruce_fence",
    "petramond:birch_fence",
    "petramond:jungle_fence",
    "petramond:acacia_fence",
    "petramond:redwood_fence",
    "petramond:pebbles_small",
    "petramond:pebbles_medium",
    "petramond:pebbles_large",
    "petramond:fallen_branch",
    "petramond:hemp",
    "petramond:fallen_branch_2",
    "petramond:fallen_branch_3",
    "petramond:lava",
    "petramond:stone_bricks",
    "petramond:stone_bricks_stairs",
    "petramond:stone_bricks_slab",
    "petramond:chiseling_station",
    "petramond:oak_trapdoor",
    "petramond:spruce_trapdoor",
    "petramond:birch_trapdoor",
    "petramond:jungle_trapdoor",
    "petramond:acacia_trapdoor",
    "petramond:redwood_trapdoor",
];

#[inline]
fn registry() -> &'static load::Registry {
    crate::content::current().blocks()
}

#[inline]
pub(super) fn all() -> &'static [Block] {
    &registry().all
}

#[inline]
pub(super) fn from_id(id: u16) -> Block {
    BlockTable::current().block(id)
}

#[inline]
pub(super) fn row<T: Copy>(table: &[T], id: u16) -> T {
    match table.get(id as usize) {
        Some(v) => *v,
        None => table[0],
    }
}

#[inline]
pub(super) fn def(block: Block) -> &'static BlockDef {
    let defs = registry().defs;
    defs.get(block.id() as usize).unwrap_or(&defs[0])
}

#[inline]
pub(super) fn shape_kind_def(kind: BlockShapeKind) -> &'static ShapeKindDef {
    &registry().shape_kinds[kind.0 as usize]
}

pub fn shape_kind_id_by_key(key: &str) -> Option<u16> {
    registry()
        .shape_kinds
        .iter()
        .position(|d| d.key == key)
        .map(|i| i as u16)
}

pub fn state_key_declared(key: &str) -> bool {
    registry()
        .shape_kinds
        .iter()
        .any(|d| d.params.state_key() == Some(key))
}

#[inline]
pub(super) fn shape_refines(id: u16) -> bool {
    BlockTable::current().refines_shape(id)
}

#[inline]
pub(super) fn shape_custom(id: u16) -> bool {
    BlockTable::current().custom_shape(id)
}

/// Dense per-id tables derived from registry rows via the normal `Block` accessors.
/// Each shape family answers for its own kind, so we can't build these inside the block
/// table's own build. That would read the table while it's still being built.
/// Each view is warmed in [`warm_views`] right after the block table, and views may read
/// each other in any order (light cells read apertures).
#[derive(Default)]
pub(crate) struct BlockViews {
    apertures: OnceLock<Box<[u32]>>,
    light_cells: OnceLock<Box<[u32]>>,
    collision: OnceLock<Box<[Option<&'static [super::Aabb]>]>>,
    nav_solid: OnceLock<Box<[bool]>>,
}

#[inline]
fn views() -> &'static BlockViews {
    &crate::content::current().block_views
}

pub(crate) fn warm_views() {
    let _ = default_light_apertures(0);
    let _ = light_cells();
    let _ = static_collision_boxes(0);
    let _ = nav_reads_solid(0);
}

#[inline]
pub(super) fn default_light_apertures(id: u16) -> u32 {
    let table = views().apertures.get_or_init(|| {
        let mut table = vec![crate::block::LIGHT_APERTURES_OPEN; all().len()].into_boxed_slice();
        for &block in all() {
            let k = block.shape_kind_def();
            table[block.id() as usize] = k.sim.light_apertures(
                &k.params,
                &crate::block::NoNeighborhood,
                crate::mathh::IVec3::ZERO,
                block,
            );
        }
        table
    });
    row(table, id)
}

#[inline]
pub fn light_cells() -> &'static [u32] {
    views().light_cells.get_or_init(|| {
        let word = |block: Block| -> u32 {
            let mut w = match block.light_shape() {
                crate::block::BlockLightShape::OpaqueCube => 0,
                crate::block::BlockLightShape::Open => crate::block::LIGHT_APERTURES_OPEN,
                crate::block::BlockLightShape::Shaped => {
                    default_light_apertures(block.id()) | super::LIGHT_CELL_SHAPED
                }
            };
            if block.transmits_direct_skylight() {
                w |= super::LIGHT_CELL_DIRECT_SKY;
            }
            w
        };
        let mut table = vec![word(Block::Air); all().len()].into_boxed_slice();
        for &block in all() {
            table[block.id() as usize] = word(block);
        }
        table
    })
}

#[inline]
pub(super) fn static_collision_boxes(id: u16) -> Option<&'static [super::Aabb]> {
    let table = views().collision.get_or_init(|| {
        let mut table: Box<[Option<&'static [super::Aabb]>]> =
            vec![None; all().len()].into_boxed_slice();
        for &block in all() {
            let k = block.shape_kind_def();
            if k.collision_state_free {
                table[block.id() as usize] = Some(k.sim.collision_boxes(
                    &k.params,
                    &crate::block::NoNeighborhood,
                    crate::mathh::IVec3::ZERO,
                    block,
                ));
            }
        }
        table
    });
    row(table, id)
}

#[inline]
pub(super) fn nav_reads_solid(id: u16) -> bool {
    let table = views().nav_solid.get_or_init(|| {
        let mut table = vec![false; all().len()].into_boxed_slice();
        for &block in all() {
            let k = block.shape_kind_def();
            table[block.id() as usize] = k.sim.nav_reads_solid(&k.params);
        }
        table
    });
    row(table, id)
}

#[inline]
pub(super) fn has_tag(id: u16, tag: super::BlockTag) -> bool {
    BlockTable::current().has_tag(id, tag)
}

#[inline]
pub(super) fn flags(id: u16) -> BlockFlags {
    BlockTable::current().flags(id)
}

#[derive(Clone, Copy)]
pub struct BlockTable(&'static load::Registry);

impl BlockTable {
    #[inline]
    pub fn current() -> BlockTable {
        BlockTable(registry())
    }

    #[inline]
    pub fn block(self, id: u16) -> Block {
        if (id as usize) < self.0.defs.len() {
            Block(id)
        } else {
            Block::Air
        }
    }

    #[inline]
    pub fn flags(self, id: u16) -> BlockFlags {
        row(&self.0.flags, id)
    }

    #[inline]
    fn def(self, id: u16) -> &'static BlockDef {
        let defs = &self.0.defs;
        defs.get(id as usize).unwrap_or(&defs[0])
    }

    #[inline]
    pub fn refines_shape(self, id: u16) -> bool {
        row(&self.0.shape_refines, id)
    }

    #[inline]
    pub fn custom_shape(self, id: u16) -> bool {
        row(&self.0.shape_custom, id)
    }

    #[inline]
    pub fn fluid(self, id: u16) -> Option<Block> {
        let flags = self.flags(id);
        if flags.fluid() {
            Some(self.block(id))
        } else if flags.contains_fluid() {
            self.def(id).contained_fluid
        } else {
            None
        }
    }

    #[inline]
    pub fn has_tag(self, id: u16, tag: super::BlockTag) -> bool {
        if tag.id() <= load::TAG_BITS_MAX {
            row(&self.0.tag_bits, id) & (1u128 << tag.id()) != 0
        } else {
            self.def(id).tags.contains(&tag)
        }
    }

    #[inline]
    pub fn emission(self, id: u16) -> u8 {
        row(&self.0.emission, id)
    }

    #[inline]
    pub fn emission_rgb(self, id: u16) -> [u8; 3] {
        row(&self.0.emission_rgb, id)
    }
}

#[inline]
pub(super) fn emission(id: u16) -> u8 {
    row(&registry().emission, id)
}

#[inline]
pub(super) fn emission_rgb(id: u16) -> [u8; 3] {
    row(&registry().emission_rgb, id)
}
