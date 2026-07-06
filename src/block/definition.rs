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
    /// Optional visual-only cube particle emitter declared by this block row. This is
    /// presentation data: it never changes simulation state and is intentionally
    /// available to mod content through `blocks.json`.
    pub particle_emitter: Option<BlockParticleEmitter>,
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
pub struct BlockParticleEmitter {
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
    /// RGB color endpoints; each particle chooses a deterministic mix between them.
    pub color: [[f32; 3]; 2],
    /// Inclusive min/max starting alpha. Lifetime fade multiplies this to zero.
    pub alpha: [f32; 2],
    /// If true, particle colors are not dimmed by sampled world light.
    #[serde(default)]
    pub fullbright: bool,
}

fn default_particle_anchor() -> ParticleEmitterAnchor {
    ParticleEmitterAnchor::BlockTop
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

    Ok(match <Rate as serde::Deserialize>::deserialize(deserializer)? {
        Rate::Fixed(rate) => [rate, rate],
        Rate::Range(range) => range,
    })
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
