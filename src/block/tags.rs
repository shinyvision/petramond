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
    "climbable",
    "snow_cover",
    "slippery",
    "melts",
]);

impl BlockTag {
    /// Any tree-leaves block: takes random ticks and decays when cut off, and
    /// counts as the support that keeps an adjacent leaf alive.
    pub const LEAVES: BlockTag = BlockTag(0);
    /// Any tree-log block: counts as support that keeps adjacent leaves alive.
    pub const LOG: BlockTag = BlockTag(1);
    /// Natural ground surface — the bare-terrain set (stone/dirt/grass/sand),
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
    /// A block the player climbs by moving into its mounted face or holding jump
    /// while their body occupies its cell — the ladder. The player physics reads
    /// it through [`World::climbable_facing_at`](crate::world::World); the climb
    /// speed and feel live in `player::movement`, never per-block.
    pub const CLIMBABLE: BlockTag = BlockTag(12);
    /// A blanket of snow covering the cell — the snow layer and the snow block.
    /// Grass renders its snowy side texture while a snow-cover block sits
    /// directly on top of it (see the mesher's grass-side branch); the look is
    /// derived from the cell above at mesh time, never stored per cell.
    pub const SNOW_COVER: BlockTag = BlockTag(13);
    /// Low-grip footing — ice and packed ice. A body standing on it glides:
    /// idle friction and directional snap both drop (the feel constants live
    /// in `player::movement`, never per-block, like the climb feel).
    pub const SLIPPERY: BlockTag = BlockTag(14);
    /// Frozen water — plain ice, NOT packed ice. Breaking it leaves a water
    /// source behind when something below can hold it (see
    /// [`Block::break_residue`]), so mining the frozen sea never leaves a dry
    /// pocket the water sim cannot refill.
    pub const MELTS: BlockTag = BlockTag(15);

    /// Resolve a `blocks.json` row tag name (see [`crate::registry::TagTable`]).
    pub(crate) fn resolve(name: &str) -> Result<BlockTag, String> {
        BLOCK_TAGS.resolve(name).map(BlockTag)
    }
}
