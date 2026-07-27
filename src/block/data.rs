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
use super::shape_kind::{BlockShapeKind, ShapeFamily, ShapeKindDef};
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

/// `Block(id)` for a registered id, `Air` past the table.
///
/// The mesher's AO ring gathers and the light flood's cube reads turn raw ids
/// into blocks tens of millions of times per world load, so this must not
/// index `defs` — that is a dependent load into a several-hundred-byte-per-row
/// table, i.e. a cache miss per cell. `defs[i].block == Block(i)` holds by
/// construction (`parse_layers` converts each row with its own id), so the
/// bounds test alone is the whole function.
#[inline]
pub(super) fn from_id(id: u8) -> Block {
    if (id as usize) < REGISTRY.defs.len() {
        Block(id)
    } else {
        Block::Air
    }
}

#[inline]
pub(super) fn def(block: Block) -> &'static BlockDef {
    &REGISTRY.defs[block.id() as usize]
}

/// The shape-kind registry row for `kind` (see [`super::shape_kind`]).
#[inline]
pub(super) fn shape_kind_def(kind: BlockShapeKind) -> &'static ShapeKindDef {
    &REGISTRY.shape_kinds[kind.0 as usize]
}

/// The session-local shape-kind id for a registry `key`, or `None` — the
/// `ResolveShape` host call's lookup. A linear scan over the small shape-kind
/// table (dozens of rows), like the other name→id resolvers.
pub(crate) fn shape_kind_id_by_key(key: &str) -> Option<u8> {
    REGISTRY
        .shape_kinds
        .iter()
        .position(|d| d.key == key)
        .map(|i| i as u8)
}

/// Whether ANY registered shape kind declares `key` as its per-cell state key
/// — the cheap gate the cell-KV write path checks before probing a
/// neighbourhood for stateful custom shapes to re-bake. Almost every KV write
/// carries an unrelated key, so the common case must cost one small-table
/// scan, not seven world reads.
pub(crate) fn state_key_declared(key: &str) -> bool {
    REGISTRY
        .shape_kinds
        .iter()
        .any(|d| d.params.state_key() == Some(key))
}

/// Dense per-id [`ShapeFamily`] — the hot shape classifier, one small-array
/// read (see the `shape_family` field on [`load::Registry`]).
#[inline]
pub(super) fn shape_family(id: u8) -> ShapeFamily {
    REGISTRY.shape_family[id as usize]
}

/// Dense per-id [`ShapeKindDef::refines`] — the refine cascade's per-cell gate
/// (see the `shape_refines` field on [`load::Registry`]). An id past the
/// registry reads `false`, matching the `Air` its `Block::from_id` resolves to.
#[inline]
pub(super) fn shape_refines(id: u8) -> bool {
    REGISTRY.shape_refines[id as usize]
}

/// Dense per-id STATE-FREE light apertures: what each block's shape blocks
/// with no world context. The light flood reads it for every `Shaped` cell the
/// sparse state gather did not cover — which is every cell of a stateless
/// shape (a cover, a cactus, a mod's plate) — so it is baked once instead of
/// re-derived per flood step.
///
/// Baked in its OWN lazy rather than inside [`REGISTRY`]: a family resolving
/// its shape reads ordinary `Block` accessors, and those read the registry, so
/// deriving this during the registry's own initialisation deadlocks it.
#[inline]
pub(super) fn default_light_apertures(id: u8) -> u32 {
    static APERTURES: LazyLock<[u32; 256]> = LazyLock::new(|| {
        let mut table = [crate::block::LIGHT_APERTURES_OPEN; 256];
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
    APERTURES[id as usize]
}

/// Dense per-id LIGHT CELL word — everything the light flood needs to know
/// about a block id, in ONE 1 KB table read.
///
/// Low 24 bits: the state-free aperture word ([`crate::block::LIGHT_APERTURES_OPEN`]
/// layout) — all zero for an opaque cube, fully open for an open cell, the
/// shape's own answer for a `Shaped` one. [`LIGHT_CELL_SHAPED`] marks the ids
/// whose per-cell state may override those bits (the only ids for which the
/// flood consults its sparse aperture gather); [`LIGHT_CELL_DIRECT_SKY`] is
/// [`Block::transmits_direct_skylight`].
///
/// The flood relaxes ~100 M edges per render-distance-12 load and each edge
/// asked two blocks for their apertures; going through `Block::light_shape`
/// meant a registry `BlockDef` load plus a virtual `ShapeSim` call per ask.
/// Baked in its own lazy for the same reason as [`default_light_apertures`].
#[inline]
pub(crate) fn light_cells() -> &'static [u32; 256] {
    static CELLS: LazyLock<[u32; 256]> = LazyLock::new(|| {
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
        // Ids past the registry resolve to `Air` through `Block::from_id`.
        let mut table = [word(Block::Air); 256];
        for &block in all() {
            table[block.id() as usize] = word(block);
        }
        table
    });
    &CELLS
}

/// Dense per-id CELL COLLISION for every shape whose boxes are fully
/// determined by the block id ([`ShapeKindDef::collision_state_free`]);
/// `None` where the shape resolves per cell (a stair corner, a fence's arms, a
/// door's swing, a box set's stored form), which must go through the world.
///
/// Every body/particle/navigation cell probe used to pay a `shape_kind_def`
/// indirection plus a virtual `collision_boxes` call for plain stone and air.
///
/// Baked in its OWN lazy for the same reason as [`default_light_apertures`].
#[inline]
pub(super) fn static_collision_boxes(id: u8) -> Option<&'static [super::Aabb]> {
    static BOXES: LazyLock<[Option<&'static [super::Aabb]>; 256]> = LazyLock::new(|| {
        let mut table: [Option<&'static [super::Aabb]>; 256] = [None; 256];
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
    BOXES[id as usize]
}

/// Dense per-id [`ShapeSim::nav_reads_solid`] — a per-KIND answer, so it bakes
/// per id like the apertures. Read once per navigation cell probe.
#[inline]
pub(super) fn nav_reads_solid(id: u8) -> bool {
    static SOLID: LazyLock<[bool; 256]> = LazyLock::new(|| {
        let mut table = [false; 256];
        for &block in all() {
            let k = block.shape_kind_def();
            table[block.id() as usize] = k.sim.nav_reads_solid(&k.params);
        }
        table
    });
    SOLID[id as usize]
}

/// Dense per-id copy of every block's [`BlockFlags`], indexed by raw block id.
///
/// The mesher/light hot loops test `is_opaque`/`occludes_ao` on neighbour ids tens of
/// times per emitted face. Going through [`def`] loads a pointer into the large
/// `BlockDef` array (≈100 rows × dozens of bytes, scattered across many cache lines) just
/// to read one flag byte. This table is 256 bytes — a handful of cache lines that stay hot
/// — so a flag query is one small-array read, not a big-struct indirection. It is derived
/// from the loaded defs by the loader, so it can never disagree with the source of truth.
/// Dense per-id tag membership; see [`load::Registry::tag_bits`].
#[inline]
pub(super) fn has_tag(id: u8, tag: super::BlockTag) -> bool {
    if tag.id() <= load::TAG_BITS_MAX {
        REGISTRY.tag_bits[id as usize] & (1u128 << tag.id()) != 0
    } else {
        REGISTRY.defs[id as usize].tags.contains(&tag)
    }
}

#[inline]
pub(super) fn flags(id: u8) -> BlockFlags {
    REGISTRY.flags[id as usize]
}

/// Dense per-id copy of every block's light `emission`, same rationale as
/// [`flags`]: the light emitter scan reads it per cell over whole sections.
#[inline]
pub(super) fn emission(id: u8) -> u8 {
    REGISTRY.emission[id as usize]
}

/// Dense per-id PER-CHANNEL light emission — `emission` split by the row's
/// `light_color`. Same rationale and same 256-entry shape as [`emission`]:
/// a block id is a `u8` everywhere it is stored, so the table covers the whole
/// id space and a lookup is one small-array read.
#[inline]
pub(super) fn emission_rgb(id: u8) -> [u8; 3] {
    REGISTRY.emission_rgb[id as usize]
}
