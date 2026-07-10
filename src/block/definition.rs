use crate::atlas::Tile;
use crate::item::DropSpec;

use super::behavior::BlockBehavior;
use super::{Aabb, Block, BlockInteraction, BlockTag, RenderShape};

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
    /// How this block is meshed — cube / cross-plant / torch. See
    /// [`Block::render_shape`](super::Block::render_shape).
    pub shape: RenderShape,
    /// Collision shape: cell-local AABBs (`&[]` = no collision). See
    /// [`Block::collision_boxes`](super::Block::collision_boxes).
    pub collision: &'static [Aabb],
    /// Block-light radiated when active, on the x2 scale (`0` = non-emitter). See
    /// [`Block::light_emission`](super::Block::light_emission).
    pub emission: u8,
    /// Optional visual-only cube particle emitter rows declared by this block row —
    /// either a referenced `particle_emitters.json` bundle's rows or one inline row.
    /// Presentation data: it never changes simulation state and is intentionally
    /// available to mod content through `blocks.json`.
    pub particle_emitter: Option<&'static [ParticleEmitter]>,
    /// Per-face tile: [top, bottom, side].
    pub tiles: [Tile; 3],
    /// Mining material class (drives tool requirement + future tool tiers).
    pub material: BlockMaterial,
    /// Minimum pickaxe tier to HARVEST this block (`0` = hand, `1` = wooden,
    /// `2` = stone, `3` = above stone). See [`Block::harvest_tier`](super::Block::harvest_tier).
    pub harvest_tier: u8,
    /// Base break time scalar in "hardness units"; `0.0` = instant, `< 0.0` =
    /// unbreakable (never a mining target). See `crate::mining` for the model.
    pub hardness: f32,
    /// What this block yields when harvested. `DropSpec::NONE` = no drop.
    pub drop: DropSpec,
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
    /// If true, particle colors are not dimmed by sampled world light.
    #[serde(default)]
    pub fullbright: bool,
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
pub(crate) enum BlockMaterial {
    None,
    Dirt,
    Sand,
    Stone,
    Ore,
    Wood,
    Wool,
    Foliage,
    Plant,
    Other,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct BlockFlags(u8);

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
    const fn contains(self, flag: BlockFlags) -> bool {
        self.0 & flag.0 == flag.0
    }
}
