use crate::atlas::Tile;
use crate::audio::Sound;
use crate::facing::Facing;
use crate::item::{DropSpec, ItemType, ToolKind};

use super::{
    data, definition, sounds, Aabb, Block, BlockBehavior, BlockFlags, BlockInteraction,
    BlockLightShape, BlockMaterial, BlockShapeKind, BlockSoundAction, BlockTag, ParticleEmitter,
    ShapeFamily, ShapeKindDef, SupportDir, ENGINE_BLOCK_NAMES,
};

impl Block {
    /// Every registered block — engine first (frozen ids), then pack-registered
    /// blocks in load order — as an id-ordered slice (`all()[id] == Block(id)`).
    pub fn all() -> &'static [Block] {
        data::all()
    }

    /// This block's composable [`BlockShapeKind`] — how it meshes / collides /
    /// places. Carries the shape [`family`](crate::block::ShapeFamily) consumers
    /// dispatch on plus the per-row [`params`](crate::block::ShapeParams).
    #[inline]
    pub fn shape_kind(self) -> BlockShapeKind {
        self.def().shape_kind
    }

    /// The shape-kind registry row for this block.
    #[inline]
    pub(crate) fn shape_kind_def(self) -> &'static ShapeKindDef {
        self.shape_kind().def()
    }

    /// The shape family this block meshes/collides as — the cheap `Copy`
    /// discriminant consumers switch on. Backed by a dense per-id LUT, so this
    /// is one small-array read (cheaper than a `def()` load).
    #[inline]
    pub fn shape_family(self) -> ShapeFamily {
        data::shape_family(self.id())
    }

    /// Whether this block's shape kind resolves refined per-cell state from its
    /// neighbours ([`ShapeKindDef::refines`]) — the edit cascade's and the
    /// load sweep's gate. Dense per-id LUT like [`shape_family`](Self::shape_family):
    /// the cascade asks it seven times per edit and the load sweep asks it once
    /// per cell of a whole section, neither of which may pay a `def()` load.
    #[inline]
    pub fn shape_refines(self) -> bool {
        Self::id_refines_shape(self.id())
    }

    /// [`shape_refines`](Self::shape_refines) straight off a raw block id — for
    /// a bulk scan over a section's id buffer, which has no `Block` in hand and
    /// must not build one per cell.
    #[inline]
    pub fn id_refines_shape(id: u8) -> bool {
        data::shape_refines(id)
    }

    /// The bbmodel kind of a [`Model`](ShapeFamily::Model) block, or `None` for
    /// any other family — the shape-kind param accessor for the model kind.
    #[inline]
    pub fn model_kind(self) -> Option<crate::block_model::BlockModelKind> {
        self.shape_kind_def().params.model_kind()
    }

    /// This block's light apertures with NO world context — what its shape
    /// blocks on its own. The flood's fallback for a `Shaped` cell that
    /// carries no per-cell state, which is every cell of a stateless shape.
    /// Consumed by the shape/light TESTS; the flood reads the same values
    /// through the fused `block::light_cells` table.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn default_light_apertures(self) -> u32 {
        data::default_light_apertures(self.id())
    }

    #[inline]
    pub(crate) fn light_shape(self) -> BlockLightShape {
        if self.is_opaque() {
            return BlockLightShape::OpaqueCube;
        }
        // The FAMILY classifies its own light participation (the engine holds
        // no per-family arms here).
        let k = self.shape_kind_def();
        k.sim.light_shape(&k.params, self)
    }

    /// Whether direct full-strength skylight can continue straight down through
    /// this cell without the normal flood-step loss. Water and leaves have open
    /// apertures, but remain filtering media rather than clear air-like cells.
    #[inline]
    pub(crate) fn transmits_direct_skylight(self) -> bool {
        self == Block::Air
            || (self.light_shape() == BlockLightShape::Open
                && self.is_transparent()
                && !self.is_water()
                && !self.is_leaves())
    }

    /// Whether replacing one block with the other leaves the light solver's
    /// inputs unchanged. Stateful partial shapes stay conservative: their
    /// block ids alone do not capture stair facing, slab occupancy, or a
    /// custom shape's per-cell baked aperture.
    pub(crate) fn has_same_light_behavior(self, other: Block) -> bool {
        let shape = self.light_shape();
        shape == other.light_shape()
            && shape != BlockLightShape::Shaped
            && self.transmits_direct_skylight() == other.transmits_direct_skylight()
            // PER CHANNEL: two emitters of equal strength but different hue are
            // not light-equivalent, and skipping the relight would leave the
            // old colour pooled on the walls.
            && self.light_emission_rgb() == other.light_emission_rgb()
    }

    /// The row's AUTHORED collision boxes, with no shape resolution — the
    /// base the default `ShapeSim::default_boxes` returns. Only the facet
    /// default should call this; everything else wants
    /// [`collision_boxes`](Self::collision_boxes).
    #[inline]
    pub(crate) fn row_collision(self) -> &'static [Aabb] {
        self.def().collision
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
        if let Some(kind) = self.model_kind() {
            return crate::block_model::collision_boxes(kind, [0, 0, 0]);
        }
        // Every other shape answers for itself: a stair's default facing, a
        // slab's half cell, a connection shape's bare post.
        let k = self.shape_kind_def();
        k.sim.default_boxes(&k.params, self)
    }

    /// This block's CELL collision when the shape's boxes are fully determined
    /// by the block id; `None` when they must be resolved against the cell (a
    /// stair corner, a fence's arms, a door's swing, a box set's stored form).
    /// The fast path behind [`World::collision_boxes_at`](crate::world::World::collision_boxes_at)
    /// and the navigation cell probes — for plain terrain it replaces a shape
    /// lookup plus a virtual resolve with one dense array read.
    #[inline]
    pub fn static_collision_boxes(self) -> Option<&'static [Aabb]> {
        data::static_collision_boxes(self.0)
    }

    /// Whether navigation reads a cell of this block as solid regardless of its
    /// real boxes (the fence family's wall rule) — the per-id bake of
    /// [`ShapeSim::nav_reads_solid`](crate::block::ShapeSim::nav_reads_solid).
    #[inline]
    pub fn nav_reads_solid(self) -> bool {
        data::nav_reads_solid(self.0)
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

    /// The visual bounding box (cell-local) for a non-full-cube block — used for
    /// the selection outline, the raycast's precise test, and the break-crack
    /// overlay so they hug the block's actual shape. `None` = an ordinary full
    /// cube (or a block with nothing to aim at), which needs no special outline.
    ///
    /// Position-LESS: the SHAPE answers for itself
    /// ([`ShapeRender::default_selection_box`]), so a walk-through thin cover
    /// and a no-collision bbmodel stay selectable while everything else
    /// outlines what it collides with. The per-cell outline of a stateful shape
    /// (a multi-cell model, a door's swung slab) is resolved by
    /// [`World::selection_box_at`](crate::world::World::selection_box_at).
    #[inline]
    pub fn visual_aabb(self) -> Option<([f32; 3], [f32; 3])> {
        let k = self.shape_kind_def();
        k.render.default_selection_box(&k.params, self)
    }

    #[inline]
    pub const fn id(self) -> u8 {
        self.0
    }

    #[inline]
    pub fn from_id(id: u8) -> Block {
        data::from_id(id)
    }

    /// Whether this is a compiled-in engine block, as opposed to one a mod
    /// pack registered at load time. Mod-registered blocks may carry mod-side
    /// rules (placement gates, hooks) the engine — and a client replica —
    /// cannot evaluate.
    #[inline]
    pub fn is_engine(self) -> bool {
        (self.0 as usize) < ENGINE_BLOCK_NAMES.len()
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
        data::has_tag(self.id(), tag)
    }

    /// Whether this is a natural terrain-solid block: the bare-ground set
    /// (`Stone`, `Dirt`, `Grass`, `Sand`) that makes up the land surface,
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

    /// Whether a face against ANOTHER CELL OF THIS SAME BLOCK is drawn at all.
    /// See [`BlockTag::MERGES_WITH_SELF`].
    #[inline]
    pub fn merges_with_self(self) -> bool {
        self.has_tag(BlockTag::MERGES_WITH_SELF)
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

    /// The next growth stage of a sapling stage row (`None` = not a sapling,
    /// or already the final stage). Growth stages are ordinary block rows; the
    /// sapling behaviour advances a cell by swapping it to this block.
    #[inline]
    pub fn next_stage(self) -> Option<Block> {
        self.def().next_stage
    }

    /// The weighted `(features.json key, weight)` tree choices a FINAL sapling
    /// stage grows into (validated at load); empty on every other row.
    #[inline]
    pub fn grows_into(self) -> &'static [(&'static str, f32)] {
        self.def().grows_into
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

    /// This block's whole dense flag word. The mesher's AO/light ring gather
    /// asks four shape questions about every ring cell; taking the word once
    /// makes them four bit tests instead of four table lookups.
    #[inline]
    pub(crate) fn flags(self) -> BlockFlags {
        data::flags(self.id())
    }

    /// Shape-class test the mesher runs per lighting-ring cell; the dense flag
    /// table answers it without a `def()` big-table read. Loader-derived from
    /// the slab family, so it cannot disagree with [`shape_family`](Self::shape_family).
    #[inline]
    pub fn is_slab(self) -> bool {
        data::flags(self.id()).is_slab()
    }

    /// Whether this block's form is a BOX SET, so its sub-cell geometry has to
    /// be asked of the shape (`ShapeRender::boxes` / `occupies_pocket`) rather
    /// than read as a whole-or-empty cell. Loader-derived from the shape
    /// kind's `resolves_to_boxes`, so it cannot disagree with it; same dense-flag
    /// rationale as [`is_slab`](Self::is_slab).
    #[inline]
    pub fn has_box_shape(self) -> bool {
        data::flags(self.id()).has_box_shape()
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

    /// Whether this block renders ALPHA-BLENDED in the transparent pass with
    /// its texture's authored alpha (ice) instead of the opaque pass's
    /// all-or-nothing cutout (glass, leaves). The mesher routes its faces
    /// into the water buffer, culls same-block shared faces (an ice sheet
    /// reads as one volume), and keeps it off the fast/greedy opaque paths.
    #[inline]
    pub fn is_translucent(self) -> bool {
        data::flags(self.id()).is_translucent()
    }

    /// Block-light this block radiates, on the SAME x2 integer scale the
    /// skylight flood-fill uses (`SKY_FULL` = 30 = full daylight = level 15). `0` for
    /// non-emitters. A torch is level 14 (`28` on the x2 scale): bright enough to
    /// light a cave, but one notch under open daylight so a lit cell still reads as
    /// "indoors" and is lit by nearby block light. Emission is pure row data —
    /// a stateful emitter (the furnace) is two rows, with the emission on the lit
    /// one, and "turning on" is a row swap. The light flood's emitter scan reads
    /// this per cell, so it goes through the dense per-id table.
    #[inline]
    pub fn light_emission(self) -> u8 {
        data::emission(self.id())
    }

    /// Block-light this block radiates PER RGB CHANNEL, on the same x2 scale as
    /// [`light_emission`](Self::light_emission): the row's `emission` split by
    /// its `light_color` hue. `[emission; 3]` for the white default, so a row
    /// that names no colour is bit-identical to the pre-colour behaviour.
    ///
    /// No channel exceeds `light_emission`, which keeps the scalar the emitter
    /// scan gates on an upper bound for every channel's flood reach.
    #[inline]
    pub fn light_emission_rgb(self) -> [u8; 3] {
        data::emission_rgb(self.id())
    }

    /// The tile drawn on the horizontal face this block's placed entity facing
    /// points to (the furnace/chest front); `None` = uniform sides. Row data,
    /// only present with the `directional_view` flag.
    #[inline]
    pub(crate) fn front_tile(self) -> Option<Tile> {
        self.def().front
    }

    /// This block's composited side face (`base` + tinted `overlay` — grass),
    /// or `None` for the ordinary single side tile.
    #[inline]
    pub(crate) fn side_overlay(self) -> Option<definition::SideOverlay> {
        self.def().side_overlay
    }

    /// The side tile swapped in while a `snow_cover` block sits directly above
    /// (snowy grass sides); `None` = sides never change with cover.
    #[inline]
    pub(crate) fn covered_side(self) -> Option<Tile> {
        self.def().covered_side
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

    /// Whether this block blankets the cell below in snow (see
    /// [`BlockTag::SNOW_COVER`]) — the mesher renders grass directly beneath
    /// such a block with its snowy side texture.
    #[inline]
    pub fn is_snow_cover(self) -> bool {
        self.has_tag(BlockTag::SNOW_COVER)
    }

    /// Whether this block is [`Fragile`](BlockTag::FRAGILE) — it shatters when it
    /// loses support or water enters its cell. Read by the water sim (a fragile cell
    /// is one water may flow into) and paired with the [`FRAGILE`](behavior) break
    /// behaviour on every fragile block's row.
    #[inline]
    pub fn is_fragile(self) -> bool {
        self.has_tag(BlockTag::FRAGILE)
    }

    /// Whether the player climbs this block (see [`BlockTag::CLIMBABLE`]).
    /// Reads the dense loader-derived flag, not the tag list — the physics
    /// probe asks every sub-step.
    #[inline]
    pub fn is_climbable(self) -> bool {
        data::flags(self.id()).is_climbable()
    }

    /// A ladder-shaped row's wall facing — the direction its panel front
    /// points, away from the supporting wall. Facing is block IDENTITY (one
    /// row per facing, like sapling stages), so the mesher, the panel
    /// collision/targeting, and the climb probe all read it off the row of
    /// the id they already fetched — no per-cell state anywhere. The default
    /// on rows that declare none (climbable non-panel rows — the exploration
    /// vines) is [`Facing::North`]; anything that would let the player ACT on
    /// the facing must use [`declared_panel_facing`](Self::declared_panel_facing)
    /// instead, or that default becomes a real direction.
    #[inline]
    pub fn panel_facing(self) -> Facing {
        self.def().panel_facing.unwrap_or_default()
    }

    /// The row's `panel_facing` as DECLARED — `None` for a row that has no
    /// wall behind it. Only the `ladder` shape may declare one (load-enforced),
    /// so this is exactly "is this a wall panel or a free-hanging block".
    #[inline]
    pub fn declared_panel_facing(self) -> Option<Facing> {
        self.def().panel_facing
    }

    /// A wall-panel's `(thickness, height)` in cell fractions: the Layer-2
    /// dimensions a mod retuned, or the engine ladder's `(1/16, full)`. Read by
    /// the ONE panel box every panel path (collision, targeting, in-world mesh)
    /// derives from, so a thicker/shorter panel stays coherent everywhere.
    #[inline]
    pub(crate) fn ladder_dims(self) -> (f32, f32) {
        self.shape_kind_def()
            .params
            .dimensions()
            .map_or((crate::ladder::THICKNESS, 1.0), |d| (d.thickness, d.height))
    }

    /// The sibling row of a wall-panel family that faces `facing`, via the
    /// placeable row's load-validated `facing_rows` map — what the shared
    /// placement commit writes for a `WallPanel` plan. A row without the map
    /// (a facing variant, or a single-facing pack row) places as itself.
    #[inline]
    pub(crate) fn wall_panel_row(self, facing: Facing) -> Block {
        match self.def().facing_rows {
            Some(rows) => rows[facing.to_u8() as usize],
            None => self,
        }
    }

    /// Whether a body standing on this block glides (see [`BlockTag::SLIPPERY`]
    /// — ice, packed ice). Reads the dense loader-derived flag, not the tag
    /// list — the physics probe asks every sub-step.
    #[inline]
    pub fn is_slippery(self) -> bool {
        data::flags(self.id()).is_slippery()
    }

    /// What the cell becomes when this block is BROKEN: air, except a
    /// [`MELTS`](BlockTag::MELTS) block (ice) leaves a water source when the
    /// cell below can hold it — solid ground or more water. Mining the frozen
    /// sea therefore refills instead of leaving a dry pocket (water never
    /// flows upward, so nothing else could); breaking ice suspended over air
    /// leaves air, never floating water. Every break path — the server break,
    /// the client's predicted break, and natural sim breaks — routes the
    /// plain-cube clear through this one rule so prediction cannot diverge.
    #[inline]
    pub fn break_residue(self, below: Block) -> Block {
        if self.has_tag(BlockTag::MELTS) && (below.is_solid() || below.is_water()) {
            Block::Water
        } else {
            Block::Air
        }
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
        // `roots_on` is the same question asked in the open vocabulary, so it
        // joins the union rather than narrowing it.
        let named = self.def().roots_on;
        if !(soil || sand || stone) && named.is_empty() {
            return true; // no substrate rule — stands on anything
        }
        (soil && ground.has_tag(BlockTag::SOIL))
            || (sand && ground.has_tag(BlockTag::SAND))
            || (stone && ground.material() == BlockMaterial::Stone)
            || named.iter().any(|t| ground.has_tag(*t))
    }

    /// Which neighbouring cell holds this block up — the row's `support`
    /// (see [`SupportDir`]). The one answer both the fragile break rule and
    /// the placement substrate gate ask, so a hanging block reads its ceiling
    /// everywhere a standing one reads its ground.
    #[inline]
    pub fn support_dir(self) -> SupportDir {
        self.def().support
    }

    /// The SHAPE its support face must have for this block to be placed (see
    /// [`RootsFace`]) — the geometric half of the substrate gate, asked
    /// alongside [`can_root_on`](Self::can_root_on)'s material half.
    #[inline]
    pub fn roots_face(self) -> crate::block::RootsFace {
        self.def().roots_face
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

    /// Whether this block cannot be hand-harvested — it still breaks, it just
    /// drops nothing without a sufficient tool of its preferred kind (Stone/Ore
    /// without a pickaxe, the snow layer without a shovel). This IS the harvest
    /// gate's condition (`harvest_tier() >= 1`, see `crate::mining::harvests`),
    /// read from the row rather than inferred from the material class.
    #[inline]
    pub fn requires_tool(self) -> bool {
        self.harvest_tier() >= 1
    }

    /// The tool kind that mines this block efficiently — a [`Pickaxe`](ToolKind::Pickaxe)
    /// for stone & ore, an [`Axe`](ToolKind::Axe) for wood (logs, planks, the
    /// crafting table, the chest), a [`Shovel`](ToolKind::Shovel) for dirt & sand
    /// (grass, podzol, gravel, clay, snow…), [`Shears`](ToolKind::Shears) for wool
    /// and plants — or `None` for blocks a bare hand mines just as fast
    /// (leaves, glass-likes). Holding the matching tool grants the tier speed-up
    /// in [`crate::mining::break_time`], and for tool-gated blocks it also
    /// unlocks the drop (see [`harvest_tier`](Self::harvest_tier)); the item half
    /// of the pairing is [`ItemType::tool`](crate::item::ItemType::tool).
    ///
    /// Plants pair with shears the way snow pairs with the shovel: almost every
    /// plant is tier 0 and hardness 0 (hand-harvested instantly, the pairing
    /// inert), but a plant row that raises `harvest_tier` to 1 becomes a
    /// CUT-ONLY yield — breaking it bare-handed destroys it dropless (short
    /// grass, whose drop feeds pasture-building for husbandry).
    #[inline]
    pub fn preferred_tool(self) -> Option<ToolKind> {
        match self.material() {
            BlockMaterial::Stone | BlockMaterial::Ore | BlockMaterial::Ice => {
                Some(ToolKind::Pickaxe)
            }
            BlockMaterial::Wood => Some(ToolKind::Axe),
            BlockMaterial::Dirt | BlockMaterial::Sand => Some(ToolKind::Shovel),
            BlockMaterial::Wool | BlockMaterial::Plant => Some(ToolKind::Shears),
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

    /// Whether targeting picks this block against its resolved collision
    /// boxes (see `ShapeRender::picks_by_boxes`) instead of a single box or a
    /// family-specific ray test.
    #[inline]
    pub fn picks_by_boxes(self) -> bool {
        let k = self.shape_kind_def();
        k.render.picks_by_boxes(&k.params)
    }

    /// The row's namespaced consumer-data entry `key` as raw JSON text, or
    /// `None` — the block interop surface (`"data"` in `blocks.json` + patch
    /// rows). Binary search over the sorted compiled slice.
    /// The cell-KV keys this block carries across break/place (the
    /// `petramond:carry` data entry, compiled at load). Empty for almost
    /// every block.
    #[inline]
    pub fn carry(self) -> &'static [&'static str] {
        self.def().carry
    }

    #[inline]
    pub fn data_value(self, key: &str) -> Option<&'static str> {
        let data = self.def().data;
        data.binary_search_by(|(k, _)| (*k).cmp(key))
            .ok()
            .map(|i| data[i].1)
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
            BlockMaterial::Sand => &sounds::SAND,
            // Leaves are hand-mined and plants pair with shears, but both are
            // plant matter and rustle alike (the Ice/Glass precedent below).
            BlockMaterial::Plant | BlockMaterial::Foliage => &sounds::LEAF,
            // Ice mines like stone (see `preferred_tool`) but SOUNDS like
            // glass — the sound follows the shatter, not the pickaxe.
            BlockMaterial::Glass | BlockMaterial::Ice => &sounds::GLASS,
            _ => &sounds::SILENT,
        }
    }

    #[inline]
    fn def(self) -> &'static definition::BlockDef {
        data::def(self)
    }
}
