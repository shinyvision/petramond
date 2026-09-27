use crate::facing::Facing;
use crate::item::DropSpec;
use crate::tile::Tile;

use super::behavior::BlockBehavior;
use super::{Aabb, Block, BlockInteraction, BlockShapeKind, BlockTag};

#[derive(Copy, Clone)]
pub(crate) struct BlockDef {
    pub block: Block,
    pub flags: BlockFlags,
    pub contained_fluid: Option<Block>,
    pub melts_to: Option<Block>,
    pub fluid: Option<&'static crate::fluid::FluidDef>,
    pub tags: &'static [BlockTag],
    pub behavior: &'static dyn BlockBehavior,
    pub interaction: BlockInteraction,
    pub shape_kind: BlockShapeKind,
    pub collision: &'static [Aabb],
    pub emission: u8,
    pub emission_rgb: [u8; 3],
    pub particle_emitter: Option<&'static [ParticleEmitter]>,
    pub tiles: [Tile; 3],
    pub uv_turns: [u8; 3],
    pub front: Option<Tile>,
    pub side_overlay: Option<SideOverlay>,
    pub covered_side: Option<Tile>,
    pub flow_tile: Option<Tile>,
    pub material: BlockMaterial,
    pub harvest_tier: u8,
    pub hardness: f32,
    pub drop: DropSpec,
    pub next_stage: Option<Block>,
    pub grows_into: &'static [(&'static str, f32)],
    pub panel_facing: Option<Facing>,
    pub animated_model: Option<&'static crate::animated_model::AnimatedModelDef>,
    pub facing_rows: Option<&'static [Block; 4]>,
    pub flipped_row: Option<Block>,
    pub rotate_y: Option<Block>,
    pub construction: Option<Construction>,
    pub data: &'static [(&'static str, &'static str)],
    pub carry: &'static [&'static str],
    pub support: SupportDir,
    pub roots_on: &'static [BlockTag],
    pub roots_face: RootsFace,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Construction {
    Item(crate::item::ItemType),
    Form(Block),
    Unsupported(&'static str),
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportDir {
    #[default]
    Below,
    Above,
    North,
    South,
    West,
    East,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootsFace {
    #[default]
    Any,
    FullCube,
    SolidFace,
}

impl RootsFace {
    pub(super) fn is_default(&self) -> bool {
        *self == RootsFace::Any
    }
}

impl SupportDir {
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

    pub fn is_wall(self) -> bool {
        !matches!(self, SupportDir::Below | SupportDir::Above)
    }

    pub(super) fn is_default(&self) -> bool {
        *self == SupportDir::Below
    }
}

#[derive(Copy, Clone)]
pub struct SideOverlay {
    pub base: Tile,
    pub overlay: Tile,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticleEmitterAnchor {
    BlockTop,
    BlockCenter,
    Local,
    TorchTop,
}

#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleEmitter {
    #[serde(default = "default_particle_anchor")]
    pub anchor: ParticleEmitterAnchor,
    #[serde(default = "default_particle_origin")]
    pub origin: [f32; 3],
    #[serde(default)]
    pub offset: [f32; 3],
    #[serde(deserialize_with = "deserialize_particle_rate")]
    pub rate: [f32; 2],
    pub lifetime: [f32; 2],
    pub size: [f32; 2],
    #[serde(default)]
    pub spawn_box: [f32; 3],
    #[serde(default)]
    pub velocity: [f32; 3],
    #[serde(default)]
    pub velocity_jitter: [f32; 3],
    #[serde(default)]
    pub color: Option<[[f32; 3]; 2]>,
    #[serde(default)]
    pub color_ramp: Option<ColorRamp>,
    pub alpha: [f32; 2],
    #[serde(default = "default_fade_power")]
    pub fade_power: f32,
    #[serde(default = "default_shrink_power")]
    pub shrink_power: f32,
    #[serde(default)]
    pub self_lit: f32,
    #[serde(default)]
    pub spiral: [f32; 2],
    #[serde(default)]
    pub requires_open: Option<SupportDir>,
    #[serde(default)]
    pub gravity: f32,
    #[serde(default)]
    pub lands: bool,
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

pub const MAX_RAMP_STOPS: usize = 6;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ColorRamp {
    stops: [[f32; 3]; MAX_RAMP_STOPS],
    len: u8,
}

impl ColorRamp {
    pub fn stops(&self) -> &[[f32; 3]] {
        &self.stops[..self.len as usize]
    }

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

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockMaterial {
    None,
    Dirt,
    Sand,
    Snow,
    Stone,
    Ore,
    Wood,
    Wool,
    Foliage,
    Plant,
    Glass,
    Ice,
    Other,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockFlags(u16);

impl BlockFlags {
    pub const FLUID: BlockFlags = BlockFlags(1 << 10);
    pub const CONTAINS_FLUID: BlockFlags = BlockFlags(1 << 12);
    pub const INVISIBLE: BlockFlags = BlockFlags(1 << 11);

    #[inline]
    pub const fn contains_fluid(self) -> bool {
        self.contains(Self::CONTAINS_FLUID)
    }
    pub const fn invisible(self) -> bool {
        self.contains(Self::INVISIBLE)
    }

    #[inline]
    pub const fn fluid(self) -> bool {
        self.contains(Self::FLUID)
    }
    pub const NONE: BlockFlags = BlockFlags(0);
    pub const SOLID: BlockFlags = BlockFlags(1 << 0);
    pub const OPAQUE: BlockFlags = BlockFlags(1 << 1);
    pub const AO_OCCLUDER: BlockFlags = BlockFlags(1 << 2);
    pub const TRANSPARENT: BlockFlags = BlockFlags(1 << 3);
    pub const SLAB: BlockFlags = BlockFlags(1 << 4);
    pub const DIRECTIONAL_VIEW: BlockFlags = BlockFlags(1 << 5);
    pub const CLIMBABLE: BlockFlags = BlockFlags(1 << 6);
    pub const SLIPPERY: BlockFlags = BlockFlags(1 << 7);
    pub const BOX_SHAPE: BlockFlags = BlockFlags(1 << 9);
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
