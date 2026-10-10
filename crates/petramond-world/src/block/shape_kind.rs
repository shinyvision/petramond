//! Block shape kinds: the composable replacement for the closed `RenderShape`
//! enum's role as `BlockDef`'s shape field.
//!
//! A [`BlockShapeKind`] is a session-local `u16` id indexing a registry of
//! [`ShapeKindDef`] rows — one row per distinct *parameterization* of a
//! [`ShapeFamily`] (all plain cubes share one row; a farmland-height and a
//! snow-height lowered cube are two rows; each bbmodel kind is its own row).
//! This mirrors [`BlockModelKind`](crate::block_model::BlockModelKind), except
//! nothing persists a shape-kind id (only block ids ride the save palette), so
//! the table is built fresh each session from the loaded block rows and its ids
//! are free to move.
//!
//! Consumers never branch on the family: every question they ask — how the
//! mesher draws it, how a ray picks it, whether it refines, what cells a
//! compound spans, how an animated model poses it — is a facet method (see
//! [`facets`]) or a field the interner precomputed from one. [`ShapeFamily`]
//! is the row's identity tag, named only inside this module (the singleton
//! table, the loader's shape vocabulary, the family-identity tests of the
//! state codecs); `workspace_names_no_shape_family_outside_shape_kind`
//! enforces that. A genuinely novel mod shape is a custom shape and
//! dispatches through the facet traits / bake cache. The per-row payloads the
//! old enum carried inline (`LoweredCube(u8)`, `Model(kind)`) live in [`ShapeParams`], so the parameter
//! variation the parameterized families need is data on the row, not a code variant.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::Aabb;
use crate::block_model::BlockModelKind;
use crate::connect;
use crate::tile::Tile;

mod corner_form;
mod custom;
pub mod facets;
pub mod families;
#[cfg(test)]
mod family_lint;
mod load;
mod neighborhood;
pub mod run_form;

pub(crate) use custom::CUSTOM_SHAPES;
pub use custom::{CustomLight, CustomShapeDef};
pub use facets::{
    full_face_at, light_aperture_face, pack_light_apertures, rests_flat_on_floor, FullFace,
    ItemRender, MeshEmitter, NoNeighborhood, PlantPlanes, RowFacts, ShapeCtx, ShapeRender,
    ShapeSim, LIGHT_APERTURES_OPEN, NO_PART_TINT,
};

pub use corner_form::{face_uv_turns, FACE_BEFORE_TURN, FRONT_AFTER_TURN};
pub use load::{RawBox, RawCustomShape, RawRun, RawShape};
pub use neighborhood::{CellCodec, CellView, ShapeNeighborhood, ShapeState, SHAPE_STATE_MAX};

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockShapeKind(pub u16);

impl BlockShapeKind {
    #[inline]
    pub fn def(self) -> &'static ShapeKindDef {
        super::data::shape_kind_def(self)
    }

    #[inline]
    pub fn family(self) -> ShapeFamily {
        self.def().family
    }

    #[inline]
    pub fn same_family(self, other: BlockShapeKind) -> bool {
        self.family() == other.family()
    }

    #[inline]
    pub fn params(self) -> &'static ShapeParams {
        &self.def().params
    }

    #[inline]
    pub fn key(self) -> &'static str {
        self.def().key
    }
}

impl std::fmt::Debug for BlockShapeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BlockShapeKind(#{})", self.0)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShapeFamily {
    Cube,
    BoxSet,
    Cross,
    Crop,
    Torch,
    Stair,
    Slab,
    Pane,
    Fence,
    Ladder,
    Model,
    Door,
    Trapdoor,
    Custom,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ShapeParams {
    None,
    BoxSet(&'static BoxSetParams),
    Model { kind: BlockModelKind },
    Connection(&'static ConnectionParams),
    Custom(&'static CustomShapeDef),
    Dimensions(&'static DimensionParams),
}

impl ShapeParams {
    #[inline]
    pub fn box_set(&self) -> Option<&'static BoxSetParams> {
        match self {
            ShapeParams::BoxSet(b) => Some(b),
            _ => None,
        }
    }

    #[inline]
    pub fn model_kind(&self) -> Option<BlockModelKind> {
        match self {
            ShapeParams::Model { kind } => Some(*kind),
            _ => None,
        }
    }

    #[inline]
    pub fn connection(&self) -> Option<&'static ConnectionParams> {
        match self {
            ShapeParams::Connection(c) => Some(c),
            _ => None,
        }
    }

    #[inline]
    pub fn state_key(&self) -> Option<&'static str> {
        self.custom().and_then(|c| c.state_key)
    }

    #[inline]
    pub fn custom(&self) -> Option<&'static CustomShapeDef> {
        match self {
            ShapeParams::Custom(c) => Some(c),
            _ => None,
        }
    }

    #[inline]
    pub fn dimensions(&self) -> Option<&'static DimensionParams> {
        match self {
            ShapeParams::Dimensions(d) => Some(d),
            _ => None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DimensionParams {
    pub inset: f32,
    pub drop: f32,
    pub thickness: f32,
    pub height: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConnectionRule {
    OpaqueOrSame,
    SolidOrSame,
    SameOnly,
    Never,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemForm {
    Segment,
    Sprite,
    Cube,
}

#[derive(Debug, PartialEq)]
pub struct ConnectionParams {
    pub post_lo: f32,
    pub post_hi: f32,
    pub rule: ConnectionRule,
    pub item_form: ItemForm,
    pub boxes: &'static [connect::Shape; 16],
}

static ENGINE_FENCE_BOXES: [connect::Shape; 16] = connect::make_shapes(6.0 / 16.0, 10.0 / 16.0);
static ENGINE_FENCE_PARAMS: ConnectionParams = ConnectionParams {
    post_lo: 6.0 / 16.0,
    post_hi: 10.0 / 16.0,
    rule: ConnectionRule::OpaqueOrSame,
    item_form: ItemForm::Segment,
    boxes: &ENGINE_FENCE_BOXES,
};
static ENGINE_PANE_BOXES: [connect::Shape; 16] = connect::make_shapes(7.0 / 16.0, 9.0 / 16.0);
static ENGINE_PANE_PARAMS: ConnectionParams = ConnectionParams {
    post_lo: 7.0 / 16.0,
    post_hi: 9.0 / 16.0,
    rule: ConnectionRule::SolidOrSame,
    item_form: ItemForm::Sprite,
    boxes: &ENGINE_PANE_BOXES,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxDef {
    pub aabb: Aabb,
    pub faces: [bool; 6],
    /// Per-face tile, `None` = the row's `[top, bottom, side]` (the plain
    /// carved-from-my-own-block case every engine box shape wants).
    ///
    /// An override is what lets ONE cell draw several distinct authored
    /// surfaces: cell-local UV pins a face's art to where the box sits in the
    /// cell, so two boxes facing the same way through the same tile can only
    /// ever show the nearer one's art. A cabinet's shelf and its counter top
    /// both face up through the same footprint — they need two tiles, not one.
    pub tiles: [Option<Tile>; 6],
    /// Whether this box is MATTER: it shadows (AO) and blocks light. `false`
    /// for a box that exists only to carry a face — the cactus's side planes
    /// span the whole cell so their faces are full width, but the body they
    /// show is the inset trunk, and treating the planes as matter would shadow
    /// the ground like a full cube and seal the cell's own light out.
    pub occludes: bool,
    pub collides: bool,
    pub double_sided: bool,
    pub casts_ao: bool,
    pub dyeable: bool,
    /// Per face: how many quarter turns the FRAME this face's art was authored
    /// in sits ahead of the box's own frame. `0` everywhere for an authored
    /// box (and for every turn of one, since a turn moves box and art
    /// together); non-zero only on a face a CORNER form inherited from the
    /// quarter-turned parent, whose art is one (or three) turns ahead.
    ///
    /// Everything frame-dependent about a face derives from this single
    /// number, which is why it replaced a parallel `front_faces` array:
    /// - the row's `front` tile belongs to the face at
    ///   `FRONT_AFTER_TURN[(shape turns + this) & 3]`, so a corner form's
    ///   front art wraps around two faces with no extra bookkeeping;
    /// - [`face_uv_turns`] must counter-rotate a `±Y` face by the TOTAL turn
    ///   `shape turns + this`, or an inherited top/bottom tile draws a quarter
    ///   turn off.
    ///
    /// Permuted by [`turned`](BoxDef::turned) like every other per-face
    /// attribute; the VALUE is a relative offset, so turning never changes it.
    pub art_turns: [u8; 6],
    pub uv: [Option<[u8; 4]>; 6],
    pub uv_turns: [u8; 6],
    pub pose: Option<crate::block::BoxPose>,
}

impl BoxDef {
    #[inline]
    pub fn face_frame_turns(&self, shape_turns: u8, face: usize) -> u8 {
        if self.pose.is_some() {
            0
        } else {
            (shape_turns + self.art_turns[face]) & 3
        }
    }

    pub fn posed_bounds(&self) -> Aabb {
        crate::block::posed_bounds(self.aabb, self.pose)
    }

    pub fn target(&self) -> crate::block::PosedBox {
        crate::block::PosedBox {
            aabb: self.aabb,
            pose: self.pose,
        }
    }

    pub fn collision_volume(&self) -> Option<Aabb> {
        if !self.collides || self.is_flat_plane() {
            return None;
        }
        self.posed_bounds().clipped_to_cell()
    }

    pub fn is_flat_plane(&self) -> bool {
        (0..3).any(|a| self.aabb.max[a] - self.aabb.min[a] <= 1e-6)
    }
}

/// The resolved parameters of a static box-set kind: the authored boxes, the
/// collision slice precomputed out of them, and their union — each in all four
/// quarter turns about Y. One per distinct authored list.
///
/// A `directional_view` row turns its shape to the facing stored at placement,
/// so every consumer needs the turned form and `collision_boxes` has to hand
/// out a `&'static [Aabb]`. Resolving the four turns at LOAD is what makes
/// that free: nothing rotates per cell, per frame, or per collision query.
/// A shape with no facing only ever reads turn `0`, and a symmetric one
/// resolves four identical (cheap, few-per-session) copies rather than making
/// every reader ask whether turning applies.
#[derive(Debug, PartialEq)]
pub struct BoxSetParams {
    forms: [[&'static [BoxDef]; 5]; 4],
    collision: [[&'static [Aabb]; 5]; 4],
    targets: [[&'static [crate::block::PosedBox]; 5]; 4],
    bounds: [[Aabb; 5]; 4],
    pub refine: BoxSetRefine,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BoxSetRefine {
    None,
    Corners,
    Run(RunParams),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RunParams {
    pub root: RunRoot,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RunRoot {
    Up,
    Down,
}

impl RunRoot {
    #[inline]
    pub fn dir(self) -> crate::mathh::IVec3 {
        match self {
            RunRoot::Up => crate::mathh::IVec3::Y,
            RunRoot::Down => crate::mathh::IVec3::NEG_Y,
        }
    }

    #[inline]
    pub fn tip(self) -> crate::mathh::IVec3 {
        -self.dir()
    }

    #[inline]
    pub fn opposite(self) -> RunRoot {
        match self {
            RunRoot::Up => RunRoot::Down,
            RunRoot::Down => RunRoot::Up,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RunRoot::Up => "up",
            RunRoot::Down => "down",
        }
    }
}

pub type RunForm = u8;
pub const RUN_TIP: RunForm = 0;
pub const RUN_FRUSTUM: RunForm = 1;
pub const RUN_MIDDLE: RunForm = 2;
pub const RUN_BASE: RunForm = 3;
pub const RUN_MERGE: RunForm = 4;

pub type CornerForm = u8;

impl BoxSetParams {
    #[inline]
    pub fn corner_joins(&self) -> bool {
        self.refine == BoxSetRefine::Corners
    }

    #[inline]
    pub fn run(&self) -> Option<RunParams> {
        match self.refine {
            BoxSetRefine::Run(r) => Some(r),
            _ => None,
        }
    }

    #[inline]
    fn form_idx(form: CornerForm) -> usize {
        if form > 4 {
            0
        } else {
            form as usize
        }
    }

    #[inline]
    pub fn boxes(&self, turns: u8, form: CornerForm) -> &'static [BoxDef] {
        self.forms[(turns & 3) as usize][Self::form_idx(form)]
    }

    #[inline]
    pub fn collision(&self, turns: u8, form: CornerForm) -> &'static [Aabb] {
        self.collision[(turns & 3) as usize][Self::form_idx(form)]
    }

    #[inline]
    pub fn targets(&self, turns: u8, form: CornerForm) -> &'static [crate::block::PosedBox] {
        self.targets[(turns & 3) as usize][Self::form_idx(form)]
    }

    #[inline]
    pub fn bounds(&self, turns: u8, form: CornerForm) -> Aabb {
        self.bounds[(turns & 3) as usize][Self::form_idx(form)]
    }
}

pub struct ShapeKindDef {
    pub key: &'static str,
    pub family: ShapeFamily,
    pub params: ShapeParams,
    pub sim: &'static dyn ShapeSim,
    pub render: &'static dyn ShapeRender,
    pub placement: &'static dyn facets::ShapePlacement,
    pub mesh_emitter: MeshEmitter,
    pub collision_state_free: bool,
    pub refines: bool,
}

impl<'de> Deserialize<'de> for RawShape {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        if let serde_json::Value::String(s) = &value {
            return match s.as_str() {
                "cube" => Ok(RawShape::Cube),
                "cross" => Ok(RawShape::Cross),
                "crop" => Ok(RawShape::Crop),
                "torch" => Ok(RawShape::Torch),
                "stair" => Ok(RawShape::Stair),
                "slab" => Ok(RawShape::Slab),
                "pane" => Ok(RawShape::Pane),
                "fence" => Ok(RawShape::Fence),
                "ladder" => Ok(RawShape::Ladder),
                "door" => Ok(RawShape::Door),
                "trapdoor" => Ok(RawShape::Trapdoor),
                other if crate::registry::is_namespaced(other) => {
                    Ok(RawShape::Named(other.to_owned()))
                }
                other => Err(D::Error::custom(format!("unknown shape '{other}'"))),
            };
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Tagged {
            Boxes(Vec<RawBox>),
            Model(BlockModelKind),
            Custom(RawCustomShape),
            Run(RawRun),
        }
        match serde_json::from_value::<Tagged>(value).map_err(D::Error::custom)? {
            Tagged::Boxes(b) => Ok(RawShape::Boxes(b)),
            Tagged::Model(kind) => Ok(RawShape::Model(kind)),
            Tagged::Custom(custom) => Ok(RawShape::Custom(custom)),
            Tagged::Run(run) => Ok(RawShape::Run(run)),
        }
    }
}

pub(super) struct ShapeKindInterner {
    table: Vec<ShapeKindDef>,
    index: HashMap<String, u16>,
}

impl ShapeKindInterner {
    pub(super) fn new() -> Self {
        Self {
            table: Vec::new(),
            index: HashMap::new(),
        }
    }

    pub(super) fn intern(
        &mut self,
        family: ShapeFamily,
        params: ShapeParams,
        key: String,
    ) -> Result<BlockShapeKind, String> {
        if let Some(&id) = self.index.get(&key) {
            return Ok(BlockShapeKind(id));
        }
        if self.table.len() >= crate::registry::WIDE_ID_CAP {
            return Err(format!(
                "too many distinct block shape kinds (4096 max) registering '{key}'"
            ));
        }
        let id = self.table.len() as u16;
        let (sim, render, placement) = families::singletons(family);
        self.table.push(ShapeKindDef {
            key: Box::leak(key.clone().into_boxed_str()),
            family,
            params,
            sim,
            render,
            placement,
            mesh_emitter: render.mesh_emitter(&params),
            collision_state_free: sim.collision_state_free(),
            refines: sim.refines(&params),
        });
        self.index.insert(key, id);
        Ok(BlockShapeKind(id))
    }

    pub(super) fn into_table(self) -> Vec<ShapeKindDef> {
        self.table
    }
}

#[cfg(test)]
mod tests;
