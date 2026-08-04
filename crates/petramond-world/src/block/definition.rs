use crate::tile::Tile;
use crate::facing::Facing;
use crate::item::DropSpec;

use super::behavior::BlockBehavior;
use super::{Aabb, Block, BlockInteraction, BlockShapeKind, BlockTag};

// No `Debug`/`PartialEq`: the `behavior` trait object is neither, and nothing
// compares or formats a whole `BlockDef` (callers read individual fields).
#[derive(Copy, Clone)]
pub(super) struct BlockDef {
    pub block: Block,
    pub flags: BlockFlags,
    /// Category memberships (see [`BlockTag`]) — what this block *is*. Most rows
    /// carry none (`&[]`); a member lists each tag it belongs to. Mirrors the
    /// item table's `tags`.
    pub tags: &'static [BlockTag],
    /// World-reactive behaviour (see [`BlockBehavior`]) — what this block *does*.
    /// Most rows are [`behavior::INERT`](super::behavior::INERT).
    pub behavior: &'static dyn BlockBehavior,
    /// What secondary-use does when the player right-clicks this placed block.
    pub interaction: BlockInteraction,
    /// How this block is meshed / collided / placed: the composable shape kind
    /// (family + parameters), interned by the loader from the row's `shape`
    /// field. See [`Block::shape_family`](super::Block::shape_family) and
    /// [`Block::shape_kind`](super::Block::shape_kind).
    pub shape_kind: BlockShapeKind,
    /// Collision shape: cell-local AABBs (`&[]` = no collision). See
    /// [`Block::collision_boxes`](super::Block::collision_boxes).
    pub collision: &'static [Aabb],
    /// Block-light radiated when active, on the x2 scale (`0` = non-emitter). See
    /// [`Block::light_emission`](super::Block::light_emission).
    pub emission: u8,
    /// [`emission`](Self::emission) split per RGB channel by the row's
    /// `light_color` hue — `[emission; 3]` for the white default. See
    /// [`Block::light_emission_rgb`](super::Block::light_emission_rgb).
    pub emission_rgb: [u8; 3],
    /// Optional visual-only cube particle emitter rows declared by this block row —
    /// either a referenced `particle_emitters.json` bundle's rows or one inline row.
    /// Presentation data: it never changes simulation state and is intentionally
    /// available to mod content through `blocks.json`.
    pub particle_emitter: Option<&'static [ParticleEmitter]>,
    /// Per-face tile: [top, bottom, side].
    pub tiles: [Tile; 3],
    /// Tile drawn on the ONE horizontal face the block's placed entity facing
    /// points to (the furnace/chest front); the other sides keep `tiles[2]`.
    /// Row-listed only with the `directional_view` flag (load-enforced) —
    /// without a stored facing there is no front to orient.
    pub front: Option<Tile>,
    /// Side compositing: side faces draw `base` with `overlay` on top, the
    /// overlay tinted by its atlas tint class (grass: dirt base + biome-tinted
    /// `grass_side_overlay`). `None` for the ordinary single side tile.
    pub side_overlay: Option<SideOverlay>,
    /// Side tile swapped in (replacing base, overlay, AND tint) while the cell
    /// directly above carries a `snow_cover` block — the snowy-grass side.
    /// Derived at mesh time from the neighbour above, never stored per cell.
    pub covered_side: Option<Tile>,
    /// Mining material class (drives tool requirement + future tool tiers).
    pub material: BlockMaterial,
    /// Minimum tool tier to HARVEST this block (`0` = hand-harvestable).
    /// Compiled from the row's `petramond:harvest` data entry.
    /// See [`Block::harvest_tier`](super::Block::harvest_tier).
    pub harvest_tier: u8,
    /// Base break time scalar in "hardness units"; `0.0` = instant, `< 0.0` =
    /// unbreakable (never a mining target). See `crate::mining` for the model.
    pub hardness: f32,
    /// What this block yields when harvested. `DropSpec::NONE` = no drop.
    pub drop: DropSpec,
    /// The block this row advances to when its growth stage completes — the
    /// stage-row chain of the `sapling` behaviour (`oak_sapling` →
    /// `oak_sapling_1` → `oak_sapling_2`). `None` on every non-sapling row
    /// and on a FINAL sapling stage, which carries [`grows_into`](Self::grows_into)
    /// instead. The loader enforces that split (see `load::convert`).
    pub next_stage: Option<Block>,
    /// The worldgen feature(s) a FINAL sapling stage grows into: weighted
    /// `(features.json key, weight)` choices, drawn by the sapling behaviour's
    /// growth roll (`world::sapling`). Empty on every other row. Keys are
    /// validated against the feature registry at load — an unknown feature is
    /// a load error, never a silent fallback tree.
    pub grows_into: &'static [(&'static str, f32)],
    /// A ladder-shaped row's fixed wall facing: the direction the panel FRONT
    /// points, away from the wall it hangs on. Facing is block IDENTITY (one
    /// row per facing, the sapling-stage pattern), so it rides the ordinary
    /// block-id save/replication lanes and never touches the entity-facing
    /// map. `Some` exactly on `shape == ladder` rows (load-enforced).
    pub panel_facing: Option<Facing>,
    /// Facing → sibling-row map ([`Facing`] discriminant order: N, S, W, E)
    /// for the PLACEABLE (item-linked) row of a wall-panel family: placement
    /// commits the sibling whose `panel_facing` matches the clicked face's
    /// normal. `None` on the non-placeable facing variants. Cross-validated
    /// at load (see `load::validate_facing_rows`).
    pub facing_rows: Option<&'static [Block; 4]>,
    /// The row's namespaced consumer-data entries (`"data"` in `blocks.json`
    /// plus every layer's `{"patch", "data"}` rows), sorted by key; each
    /// value is the entry's canonical raw JSON text. The block interop
    /// surface — read via `Block::data_value` and the
    /// `BlockDataGet`/`BlocksWithData` host calls.
    pub data: &'static [(&'static str, &'static str)],
    /// Cell-KV keys this block carries across break/place: on break the
    /// listed entries copy into the drop's per-stack instance data, on place
    /// they copy back into cell KV (the `petramond:carry` data key, declared
    /// by whichever pack owns the carried vocabulary — the engine is a
    /// key-agnostic courier). Empty for almost every block.
    pub carry: &'static [&'static str],
    /// Which neighbouring cell holds this block up (see [`SupportDir`]).
    pub support: SupportDir,
    /// Ground tags this block accepts to be PLACED on, ANY of which satisfies
    /// it — the open-vocabulary half of the substrate gate, so a pack declares
    /// "I grow on whatever carries my own `ns:tag`" without the engine
    /// learning the category. Empty on almost every row; combines with the
    /// `RootsIn*` tags by union (see [`Block::can_root_on`](super::Block::can_root_on)).
    pub roots_on: &'static [BlockTag],
    /// What the SUPPORT cell's face toward this block has to look like for a
    /// placement to be allowed (see [`RootsFace`]).
    pub roots_face: RootsFace,
}

/// Which neighbouring cell a block's SUPPORT is in: the cell that has to hold
/// something for it to stay put, and the cell its placement substrate gate
/// reads.
///
/// A row DECLARES this; nothing derives it from a family or a block id. A
/// standing plant is held from below, a hanging block from above, and the
/// engine never learns which rows are which.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportDir {
    /// The cell BELOW — the ground a plant roots in. What every row that
    /// declares nothing keeps.
    #[default]
    Below,
    /// The cell ABOVE — the ceiling a hanging block grows down from.
    Above,
    /// A WALL: the horizontal neighbour named holds this block up — a bracket,
    /// a wall lamp, a sign. Named by the DIRECTION OF THE SUPPORT from the
    /// block, so a row whose back is against the west wall declares `"west"`.
    ///
    /// Four separate variants rather than one carrying a [`Facing`]: the field
    /// is authored in JSON, and the loader's `rename_all = "snake_case"` turns
    /// these into the same flat vocabulary as `above`/`below`.
    ///
    /// [`Facing`]: crate::facing::Facing
    North,
    South,
    West,
    East,
}

/// A GEOMETRIC requirement on the face a placement rests against — the
/// companion of the `RootsIn*` tags, which ask what the support is MADE of.
///
/// The two axes are independent on purpose: "grows on fertile ground" is
/// membership and belongs in tags, "needs something whole under it" is shape
/// and cannot be a tag at all, since one row's cell answers differently
/// depending on the state it resolved to (a stair's top face, a fence post's).
/// The family answers through [`ShapeSim::full_face`](super::ShapeSim), so a
/// row that declares this learns nothing about which families exist.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootsFace {
    /// No shape requirement — what every row that declares nothing keeps.
    #[default]
    Any,
    /// A complete face of UNSHAPED matter: the support must answer
    /// [`FullFace::Cube`](super::FullFace) and be opaque, so planks, stone and
    /// grass hold the block while a fence, stair, slab, glass pane — or air —
    /// do not. Opacity is the same material rule a wall mount applies to a
    /// cube face; without it the block would take root on glass and leaves.
    FullCube,
    /// A complete face of ANY shape — the mounted-face rule a wall torch uses,
    /// applied to whichever cell the row's `support` names. An opaque cube's
    /// face, a stair's flat side, a slab's top, a counter's worktop; NOT a
    /// flower, a torch, or the mid-cell top of a bottom slab, which are not
    /// faces at all.
    ///
    /// The looser sibling of [`FullCube`](Self::FullCube), and the one to
    /// reach for when the requirement is "something whole to stand ON"
    /// rather than "unshaped ground to root IN". Completeness is answered by
    /// the shape itself (`facets::face_is_solid` for the arbitrary-matter
    /// families), so no list of acceptable supports exists to maintain.
    SolidFace,
}

impl RootsFace {
    /// Whether this is the `any` every silent row carries (the loader's
    /// round-trip skip).
    pub(super) fn is_default(&self) -> bool {
        *self == RootsFace::Any
    }
}

impl SupportDir {
    /// The support cell of a block occupying `pos`.
    pub fn support_cell(self, pos: crate::mathh::IVec3) -> crate::mathh::IVec3 {
        match self {
            SupportDir::Below => pos - crate::mathh::IVec3::Y,
            SupportDir::Above => pos + crate::mathh::IVec3::Y,
            SupportDir::North => pos + crate::facing::Facing::North.dir(),
            SupportDir::South => pos + crate::facing::Facing::South.dir(),
            SupportDir::West => pos + crate::facing::Facing::West.dir(),
            SupportDir::East => pos + crate::facing::Facing::East.dir(),
        }
    }

    /// Whether the support is a WALL rather than a floor or a ceiling — the
    /// case whose accept rule is the shared mounted-face test.
    pub fn is_wall(self) -> bool {
        !matches!(self, SupportDir::Below | SupportDir::Above)
    }

    /// Whether this is the `below` every silent row carries (the loader's
    /// round-trip skip).
    pub(super) fn is_default(&self) -> bool {
        *self == SupportDir::Below
    }
}

/// A row's composited side face: `base` drawn untinted with `overlay` blended
/// on top (the overlay takes the vertex tint — see the mesher's overlay lane).
#[derive(Copy, Clone)]
pub struct SideOverlay {
    pub base: Tile,
    pub overlay: Tile,
}

/// Where a block-row particle emitter starts from inside the occupied cell.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticleEmitterAnchor {
    /// Top center of the block cell: `(0.5, 1.0, 0.5)`.
    BlockTop,
    /// Center of the block cell: `(0.5, 0.5, 0.5)`.
    BlockCenter,
    /// The `origin` vector from the emitter row.
    Local,
    /// The actual rendered torch pole tip, including wall-torch tilt.
    TorchTop,
}

/// Visual-only cube particle emitter data owned by a block definition.
///
/// A content pack opts in by adding `particle_emitter` to its `blocks.json` row.
/// The renderer derives short-lived particles from this immutable row and loaded
/// block positions; no particle state is saved and no mod code needs to run.
#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleEmitter {
    /// Emitter anchor. Defaults to top center for ordinary block emitters.
    #[serde(default = "default_particle_anchor")]
    pub anchor: ParticleEmitterAnchor,
    /// Cell-local origin used when `anchor = "local"`.
    #[serde(default = "default_particle_origin")]
    pub origin: [f32; 3],
    /// Offset added after anchor resolution, in block units.
    #[serde(default)]
    pub offset: [f32; 3],
    /// Inclusive min/max particles spawned per second. JSON may use a single number
    /// for a fixed-rate emitter or `[min, max]` for irregular spawn timing.
    #[serde(deserialize_with = "deserialize_particle_rate")]
    pub rate: [f32; 2],
    /// Inclusive min/max particle lifetime in seconds.
    pub lifetime: [f32; 2],
    /// Inclusive min/max cube edge length in block units.
    pub size: [f32; 2],
    /// Spawn jitter half-extents around the anchor, in block units.
    #[serde(default)]
    pub spawn_box: [f32; 3],
    /// Base particle velocity, in blocks per second.
    #[serde(default)]
    pub velocity: [f32; 3],
    /// Per-axis random velocity jitter, in blocks per second.
    #[serde(default)]
    pub velocity_jitter: [f32; 3],
    /// RGB color endpoints; each particle chooses a deterministic mix between
    /// them AT BIRTH and keeps it. Exactly one of `color` / `color_ramp` must
    /// be declared.
    #[serde(default)]
    pub color: Option<[[f32; 3]; 2]>,
    /// Color OVER LIFE: 2..=6 RGB stops sampled by age fraction, so a rising
    /// particle cools through the ramp (white-hot → yellow → orange → red →
    /// charcoal reads as real fire, since age maps to height). Exactly one of
    /// `color` / `color_ramp` must be declared.
    #[serde(default)]
    pub color_ramp: Option<ColorRamp>,
    /// Inclusive min/max starting alpha. Lifetime fade multiplies this to zero.
    pub alpha: [f32; 2],
    /// Exponent of the alpha fade over life: `alpha *= (1 - t)^fade_power`.
    /// The default `2` is the classic quick fade; `1` keeps late-life
    /// particles visible longer (charcoal/smoke tips that linger, like the
    /// dark cubes atop a fire).
    #[serde(default = "default_fade_power")]
    pub fade_power: f32,
    /// Exponent of the size shrink over life: `size *= (1 - t)^shrink_power`.
    /// The default `1` is the classic linear shrink; lower keeps late-life
    /// cubes chunky until they pop away — without it, a `color_ramp`'s cool
    /// (red/charcoal) end shrinks into invisibility before it reads.
    #[serde(default = "default_shrink_power")]
    pub shrink_power: f32,
    /// How much of its brightness the particle provides ITSELF, `0..=1`. The
    /// light sampled at the anchor is mixed toward full bright by this
    /// fraction: `0` (the default) is an ordinary lit particle that goes black
    /// in an unlit cave, `1` ignores world light entirely (flames, sparks), and
    /// an intermediate value still dims with the room but bottoms out above the
    /// cave floor — a mote that reads as faintly luminous without lying about
    /// how lit the room is.
    #[serde(default)]
    pub self_lit: f32,
    /// `[radius, revolutions_per_second]` — each particle orbits the emitter's
    /// vertical axis while it rises, so a column of particles twirls upward.
    /// Both values are OUTER/NOMINAL: every particle deterministically draws its
    /// own orbit radius (60-100% of `radius`) and angular speed (50-150% of the
    /// nominal), so the column reads organic rather than as a rigid helix.
    /// `radius == 0` (the default) disables it; negative revolutions spin the
    /// other way.
    #[serde(default)]
    pub spiral: [f32; 2],
}

fn default_particle_anchor() -> ParticleEmitterAnchor {
    ParticleEmitterAnchor::BlockTop
}

fn default_fade_power() -> f32 {
    2.0
}

fn default_shrink_power() -> f32 {
    1.0
}

/// Most stops a `color_ramp` may declare.
pub const MAX_RAMP_STOPS: usize = 6;

/// A color-over-life ramp: evenly spaced RGB stops sampled by age fraction.
/// Fixed-capacity so emitter rows stay `Copy`; serde speaks a plain JSON list
/// of 2..=[`MAX_RAMP_STOPS`] stops.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ColorRamp {
    stops: [[f32; 3]; MAX_RAMP_STOPS],
    len: u8,
}

impl ColorRamp {
    /// The declared stops, in order.
    pub fn stops(&self) -> &[[f32; 3]] {
        &self.stops[..self.len as usize]
    }

    /// The ramp color at age fraction `t` (clamped to `0..=1`), linearly
    /// interpolated between the two surrounding stops.
    pub fn sample(&self, t: f32) -> [f32; 3] {
        let n = self.len as usize;
        let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (x as usize).min(n - 2);
        let f = x - i as f32;
        let (a, b) = (self.stops[i], self.stops[i + 1]);
        [
            a[0] + (b[0] - a[0]) * f,
            a[1] + (b[1] - a[1]) * f,
            a[2] + (b[2] - a[2]) * f,
        ]
    }
}

impl serde::Serialize for ColorRamp {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.stops().serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for ColorRamp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let listed = Vec::<[f32; 3]>::deserialize(d)?;
        if !(2..=MAX_RAMP_STOPS).contains(&listed.len()) {
            return Err(serde::de::Error::custom(format!(
                "color_ramp needs 2..={MAX_RAMP_STOPS} stops, got {}",
                listed.len()
            )));
        }
        let mut stops = [[0.0; 3]; MAX_RAMP_STOPS];
        stops[..listed.len()].copy_from_slice(&listed);
        Ok(ColorRamp {
            stops,
            len: listed.len() as u8,
        })
    }
}

fn default_particle_origin() -> [f32; 3] {
    [0.5, 1.0, 0.5]
}

fn deserialize_particle_rate<'de, D>(deserializer: D) -> Result<[f32; 2], D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Rate {
        Fixed(f32),
        Range([f32; 2]),
    }

    Ok(
        match <Rate as serde::Deserialize>::deserialize(deserializer)? {
            Rate::Fixed(rate) => [rate, rate],
            Rate::Range(range) => range,
        },
    )
}

/// Mining material class of a block — an internal mining-grouping key (drives the
/// tool requirement and groups blocks for tool tiers). Not part of the public
/// surface: callers use [`Block::requires_tool`](super::Block::requires_tool) /
/// [`Block::harvest_tier`](super::Block::harvest_tier) instead.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockMaterial {
    None,
    Dirt,
    Sand,
    Stone,
    Ore,
    Wood,
    Wool,
    Foliage,
    Plant,
    /// Brittle vitreous blocks — glass and panes. Hand-mined (no preferred
    /// tool), shatters with the glass sound set.
    Glass,
    /// Frozen water — ice and packed ice. Pickaxe-classed like stone, but
    /// shatters with the glass sound set.
    Ice,
    Other,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockFlags(u16);

impl BlockFlags {
    /// No material properties at all (air). Replaceability is no longer a flag —
    /// it migrated to [`BlockTag::REPLACEABLE`](super::BlockTag::REPLACEABLE).
    pub const NONE: BlockFlags = BlockFlags(0);
    pub const SOLID: BlockFlags = BlockFlags(1 << 0);
    pub const OPAQUE: BlockFlags = BlockFlags(1 << 1);
    pub const AO_OCCLUDER: BlockFlags = BlockFlags(1 << 2);
    pub const TRANSPARENT: BlockFlags = BlockFlags(1 << 3);
    /// Derived by the loader from `shape == slab`, never listed in a data row. The
    /// mesher's per-ring-cell "is this a full slab stack" test needs the shape class
    /// without a `def()` big-table read, same rationale as the rest of this table.
    pub const SLAB: BlockFlags = BlockFlags(1 << 4);
    pub const DIRECTIONAL_VIEW: BlockFlags = BlockFlags(1 << 5);
    /// Derived by the loader from the `climbable` tag, never listed in a data
    /// row. The player physics probes "is this cell climbable" every sub-step
    /// and needs the answer without a `def()` big-table read, same rationale
    /// as [`SLAB`](Self::SLAB).
    pub const CLIMBABLE: BlockFlags = BlockFlags(1 << 6);
    /// Derived by the loader from the `slippery` tag, never listed in a data
    /// row. The player physics probes the support block's grip every sub-step,
    /// same rationale as [`CLIMBABLE`](Self::CLIMBABLE).
    pub const SLIPPERY: BlockFlags = BlockFlags(1 << 7);
    /// Derived by the loader from the shape kind's `resolves_to_boxes`, never
    /// listed in a data row: this block's form is a BOX SET, so a consumer
    /// that cares about sub-cell geometry must ask the shape instead of
    /// reading the cell as whole-or-empty. The mesher's AO ring gathers test
    /// it per neighbour cell and the exposure masks per pad cell, so it needs
    /// the answer without a `def()` big-table read, same rationale as
    /// [`SLAB`](Self::SLAB).
    pub const BOX_SHAPE: BlockFlags = BlockFlags(1 << 9);
    /// Row-listed: this block renders ALPHA-BLENDED in the transparent pass
    /// with its texture's own authored alpha (ice) — unlike `transparent`
    /// cutout blocks (glass), whose texels are all-or-nothing and render in
    /// the opaque pass. A translucent row must not be `opaque` (faces behind
    /// it stay visible) and its texture must sit BELOW the cutout threshold
    /// (see `block_tiles_match_their_render_pass_alpha_contract`'s translucent half).
    pub const TRANSLUCENT: BlockFlags = BlockFlags(1 << 8);

    #[inline]
    pub const fn with(self, flag: BlockFlags) -> BlockFlags {
        BlockFlags(self.0 | flag.0)
    }

    #[inline]
    pub const fn is_solid(self) -> bool {
        self.contains(BlockFlags::SOLID)
    }

    #[inline]
    pub const fn is_opaque(self) -> bool {
        self.contains(BlockFlags::OPAQUE)
    }

    #[inline]
    pub const fn occludes_ao(self) -> bool {
        self.contains(BlockFlags::AO_OCCLUDER)
    }

    #[inline]
    pub const fn is_transparent(self) -> bool {
        self.contains(BlockFlags::TRANSPARENT)
    }

    #[inline]
    pub const fn is_directional_view(self) -> bool {
        self.contains(BlockFlags::DIRECTIONAL_VIEW)
    }

    #[inline]
    pub const fn is_slab(self) -> bool {
        self.contains(BlockFlags::SLAB)
    }

    #[inline]
    pub const fn is_climbable(self) -> bool {
        self.contains(BlockFlags::CLIMBABLE)
    }

    #[inline]
    pub const fn is_slippery(self) -> bool {
        self.contains(BlockFlags::SLIPPERY)
    }

    #[inline]
    pub const fn is_translucent(self) -> bool {
        self.contains(BlockFlags::TRANSLUCENT)
    }

    #[inline]
    pub const fn has_box_shape(self) -> bool {
        self.contains(BlockFlags::BOX_SHAPE)
    }

    #[inline]
    const fn contains(self, flag: BlockFlags) -> bool {
        self.0 & flag.0 == flag.0
    }
}
