//! Block shape kinds: the composable replacement for the closed `RenderShape`
//! enum's role as `BlockDef`'s shape field.
//!
//! A [`BlockShapeKind`] is a session-local `u8` id indexing a registry of
//! [`ShapeKindDef`] rows — one row per distinct *parameterization* of a
//! [`ShapeFamily`] (all plain cubes share one row; a farmland-height and a
//! snow-height lowered cube are two rows; each bbmodel kind is its own row).
//! This mirrors [`BlockModelKind`](crate::block_model::BlockModelKind), except
//! nothing persists a shape-kind id (only block ids ride the save palette), so
//! the table is built fresh each session from the loaded block rows and its ids
//! are free to move.
//!
//! Consumers dispatch on the cheap [`ShapeFamily`] enum (`Block::shape_family`)
//! exactly where they used to match `RenderShape`; a genuinely novel mod shape
//! is [`ShapeFamily::Custom`] and dispatches through the facet traits / bake
//! cache. The per-row payloads the old enum carried inline
//! (`LoweredCube(u8)`, `Model(kind)`) live in [`ShapeParams`], so the parameter
//! variation the parameterized families need is data on the row, not a code variant.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::Aabb;
use crate::tile::Tile;
use crate::block_model::BlockModelKind;
use crate::connect;

mod corner_form;
mod custom;
pub mod facets;
pub mod families;
mod load;
mod neighborhood;

pub use custom::{CustomLight, CustomShapeDef};
pub use facets::{
    full_face_at, light_aperture_face, pack_light_apertures, rests_flat_on_floor, FullFace,
    ItemRender, NoNeighborhood, ShapeCtx, ShapeRender, ShapeSim, LIGHT_APERTURES_OPEN,
    NO_PART_TINT,
};

pub use corner_form::{face_uv_turns, FRONT_AFTER_TURN};
pub use load::{family_resolves_to_boxes, RawBox, RawCustomShape, RawShape};
pub use neighborhood::{CellCodec, CellView, ShapeNeighborhood, ShapeState, SHAPE_STATE_MAX};

/// A block shape kind — a session-local id into the [`ShapeKindDef`] table
/// (`shape_kind_def`). One id per distinct `(family, params)`; the id replaces
/// `RenderShape` as `BlockDef`'s shape field. Not persisted, so unlike
/// [`Block`](super::Block) its numeric value is free to change between sessions.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockShapeKind(pub u8);

impl BlockShapeKind {
    /// The registry row for this kind.
    #[inline]
    pub fn def(self) -> &'static ShapeKindDef {
        super::data::shape_kind_def(self)
    }

    /// The shape family this kind belongs to — the cheap `Copy` discriminant
    /// consumers match on (the `RenderShape`-match replacement).
    #[inline]
    pub fn family(self) -> ShapeFamily {
        self.def().family
    }

    /// This kind's parameters.
    #[inline]
    pub fn params(self) -> &'static ShapeParams {
        &self.def().params
    }

    /// The canonical registry key (diagnostics + parameterized/custom lookup).
    #[inline]
    pub fn key(self) -> &'static str {
        self.def().key
    }
}

impl std::fmt::Debug for BlockShapeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Numeric only — the id is session-local and Debug must not depend on
        // the lazily-built registry being ready (it prints mid-bootstrap).
        write!(f, "BlockShapeKind(#{})", self.0)
    }
}

/// The shape families the engine meshes/collides/places. This is the closed set
/// consumers switch on (what `RenderShape`'s variants were), minus the inline
/// payloads (which moved to [`ShapeParams`]) and plus [`Custom`](Self::Custom)
/// for mod-defined procedural shapes. A mod never adds a variant here:
/// a parameterized shape reuses an existing family with different [`ShapeParams`], and
/// a custom shape is `Custom`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShapeFamily {
    Cube,
    /// A static list of axis-aligned boxes authored as data — farmland, the
    /// snow layer, the cactus, a mod's dirt path. The ONE family for a fixed
    /// sub-cell shape; anything neighbour-dependent is a resolver family, and
    /// anything procedural is [`Custom`](Self::Custom).
    BoxSet,
    Cross,
    Crop,
    Torch,
    Stair,
    Slab,
    Pane,
    Fence,
    /// A thin climbable/decorative wall panel (the engine ladder). Named for the
    /// generalised parameterized family; the engine's only member is the ladder.
    Ladder,
    Model,
    Door,
    /// A mod-defined procedural shape, meshed/collided from the WASM bake
    /// cache. The [`ShapeParams::Custom`] payload carries its declaration.
    Custom,
}

/// The per-row parameters of a shape kind — what the old `RenderShape`
/// variants carried inline, plus the parameterized family dimensions and the
/// custom-shape declaration. Most engine rows are [`None`](Self::None) (the
/// family alone fully describes them).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ShapeParams {
    /// No parameters — the family is self-describing (cube, cross, torch,
    /// stair, slab, door, ladder).
    None,
    /// A static box set's authored boxes (`{"boxes": [...]}`), behind a
    /// `&'static` so [`ShapeParams`] stays a cheap `Copy`.
    BoxSet(&'static BoxSetParams),
    /// A bbmodel block's model kind.
    Model { kind: BlockModelKind },
    /// A parameterized connection shape (fence or pane): the post dimensions,
    /// connection rule, item form, and the precomputed box table. Behind a
    /// `&'static` so [`ShapeParams`] stays a cheap `Copy` — the engine defaults
    /// are statics, a mod's `{"custom": …}` shape leaks its table once at load.
    Connection(&'static ConnectionParams),
    /// A mod-declared procedural shape: a reference to its
    /// `shapes.json` declaration. The geometry comes from the pack's WASM bake
    /// (cached per section); this carries the static metadata the engine reads
    /// without dispatching, and the fallback a trapped bake freezes to.
    Custom(&'static CustomShapeDef),
    /// A parameterized cross / crop / wall-panel: the numeric
    /// dimensions a mod retuned (a tighter crop lattice, a thicker panel) with no
    /// WASM. Behind a `&'static` so [`ShapeParams`] stays cheap `Copy`.
    Dimensions(&'static DimensionParams),
}

impl ShapeParams {
    /// The authored boxes, if this is a static box-set kind.
    #[inline]
    pub fn box_set(&self) -> Option<&'static BoxSetParams> {
        match self {
            ShapeParams::BoxSet(b) => Some(b),
            _ => None,
        }
    }

    /// The bbmodel kind, if this is a model kind.
    #[inline]
    pub fn model_kind(&self) -> Option<BlockModelKind> {
        match self {
            ShapeParams::Model { kind } => Some(*kind),
            _ => None,
        }
    }

    /// The connection parameters, if this is a fence/pane kind.
    #[inline]
    pub fn connection(&self) -> Option<&'static ConnectionParams> {
        match self {
            ShapeParams::Connection(c) => Some(c),
            _ => None,
        }
    }

    /// The per-cell state key a custom shape declares (`shapes.json`
    /// `"state_key"`), or `None` — the cell-KV key the bake input carries for
    /// the cell and its neighbours (see [`mod_api::CellInput`]).
    #[inline]
    pub fn state_key(&self) -> Option<&'static str> {
        self.custom().and_then(|c| c.state_key)
    }

    /// The custom-shape declaration, if this is a custom shape.
    #[inline]
    pub fn custom(&self) -> Option<&'static CustomShapeDef> {
        match self {
            ShapeParams::Custom(c) => Some(c),
            _ => None,
        }
    }

    /// The parameterized render/collision dimensions, if this is a parameterized
    /// cross / crop / wall-panel kind.
    #[inline]
    pub fn dimensions(&self) -> Option<&'static DimensionParams> {
        match self {
            ShapeParams::Dimensions(d) => Some(d),
            _ => None,
        }
    }
}

/// The parameterized render/collision dimensions of a cross / crop / wall-panel kind —
/// the numeric slice a mod may retune with no WASM. Every field is a CELL
/// FRACTION (`0.0..1.0`); a family reads only the fields it uses and the engine
/// defaults reproduce the hardcoded shapes exactly.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DimensionParams {
    /// Cross/crop billboard-plane inset from the cell edge.
    pub inset: f32,
    /// Crop lattice vertical drop (how far it sinks toward the floor).
    pub drop: f32,
    /// Wall-panel slab thickness (flush against its wall).
    pub thickness: f32,
    /// Wall-panel visible height from the floor.
    pub height: f32,
}

/// How a connection shape (fence / pane / wall) decides whether to grow an arm
/// toward a neighbour. The rule is a `params` field so a mod's wall or bar picks
/// its own without new code.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConnectionRule {
    /// Opaque full cubes, same-family shapes (any params), full-face stairs,
    /// full slab stacks — the engine fence rule.
    OpaqueOrSame,
    /// Solid full cubes (glass included, minus `no_pane_connect`), same-family
    /// shapes, full-face stairs, full slab stacks — the engine pane rule.
    SolidOrSame,
    /// Only same-family shapes join; cubes/stairs/slabs never do.
    SameOnly,
    /// Never connects — a bare post.
    Never,
}

/// What a connection shape looks like as an item (icon / dropped / in-hand) —
/// a connection shape never shows its connected form, so it must declare which
/// canonical form its item takes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemForm {
    /// A fixed no-neighbour segment built from the family + params (fence, wall).
    Segment,
    /// The item's own flat/extruded sprite (pane, bars).
    Sprite,
    /// A full-cube icon.
    Cube,
}

/// The resolved parameters of a connection shape (fence / pane / wall): the post
/// extent, the connection rule, the item form, and the precomputed 16-mask
/// collision/selection box table (post + full-height arms, from
/// [`connect::make_shapes`]). One per distinct shape kind.
#[derive(Debug, PartialEq)]
pub struct ConnectionParams {
    /// Post low/high extent on both horizontal axes (cell fraction `0..1`).
    pub post_lo: f32,
    pub post_hi: f32,
    pub rule: ConnectionRule,
    pub item_form: ItemForm,
    /// The 16 collision/selection box sets, one per connection mask.
    pub boxes: &'static [connect::Shape; 16],
}

// Engine connection defaults, held as statics so the many engine fence/pane
// rows resolve without leaking a fresh table each (only a mod's custom shape
// leaks). The dimensions match the historical `crate::fence` / `crate::pane`
// consts exactly (6/16..10/16 fence post, 7/16..9/16 pane post).
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

/// One authored box of a `{"boxes": [...]}` shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxDef {
    /// Cell-local extent (`0.0..1.0` per axis).
    pub aabb: Aabb,
    /// Which of the six faces this box DRAWS, in canonical face order
    /// (`+X, -X, +Y, -Y, +Z, -Z`). A `false` face is one the shape NEVER
    /// emits whatever the geometry says — the cactus's cap plates draw only
    /// their outward cap, so the trunk shows no rim.
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
    /// Whether this box obstructs movement. Independent of `occludes`: a snow
    /// cover is matter you walk through, a face plane is neither.
    pub collides: bool,
    /// Draw the box's faces from both sides — see
    /// [`ShapeBox::double_sided`](crate::block::ShapeBox).
    pub double_sided: bool,
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
    bounds: [[Aabb; 5]; 4],
    /// Whether this kind resolves CORNER forms from its perpendicular
    /// same-kind neighbours (the row's `"corners": true`) — the stair rule
    /// lifted from quadrant masks to box lists. `false` = the shape has one
    /// form per turn and never refines.
    pub corner_joins: bool,
}

/// A corner-joining cell's resolved form (stored in cell-state byte 1; byte 0
/// stays the placed facing — the stair's identity/refined split):
/// [`STRAIGHT`](CornerForm) `0`, outer corner `1`/`2`, inner corner `3`/`4`,
/// where odd = the perpendicular neighbour faces one quarter turn CLOCKWISE of
/// this cell and even = counter-clockwise. Resolved by the same neighbour rule
/// stairs use; an out-of-range stored byte reads as straight.
pub type CornerForm = u8;

impl BoxSetParams {
    /// An out-of-vocabulary stored byte (an old world's stale state, until
    /// the load sweep rewrites it) reads as STRAIGHT — never a wrong corner.
    #[inline]
    fn form_idx(form: CornerForm) -> usize {
        if form > 4 {
            0
        } else {
            form as usize
        }
    }

    /// The drawn boxes at `turns` quarter turns about Y (`0` = as authored)
    /// in corner form `form`.
    #[inline]
    pub fn boxes(&self, turns: u8, form: CornerForm) -> &'static [BoxDef] {
        self.forms[(turns & 3) as usize][Self::form_idx(form)]
    }

    /// The `collides` boxes, ready to hand out.
    #[inline]
    pub fn collision(&self, turns: u8, form: CornerForm) -> &'static [Aabb] {
        self.collision[(turns & 3) as usize][Self::form_idx(form)]
    }

    /// The union of every DRAWN box — the selection outline and target box.
    #[inline]
    pub fn bounds(&self, turns: u8, form: CornerForm) -> Aabb {
        self.bounds[(turns & 3) as usize][Self::form_idx(form)]
    }
}

/// One shape-kind registry row: the family, its canonical key, the parameters
/// that distinguish this kind from others of the same family, and the facet
/// singletons consumers dispatch through.
pub struct ShapeKindDef {
    /// Canonical key — `petramond:<family>` for a parameterless engine kind,
    /// `petramond:lowered_cube/<n>` / `petramond:model/<model_key>` for the
    /// parameterized engine kinds, or a `mod_id:name` for a parameterized or custom kind.
    pub key: &'static str,
    pub family: ShapeFamily,
    pub params: ShapeParams,
    /// Deterministic sim behavior (collision, support, nav).
    pub sim: &'static dyn ShapeSim,
    /// Client presentation behavior (selection outline, item form).
    pub render: &'static dyn ShapeRender,
    /// Placement behavior (which cells the write lands in, what state it
    /// writes) — the seam that replaced the engine's per-family placement
    /// match.
    pub placement: &'static dyn facets::ShapePlacement,
    /// Whether this family answers [`ShapeRender::boxes`] — the mesher's
    /// per-cell gate. A plain field so the hot loop reads it without a
    /// virtual call; set from the family at intern time.
    pub resolves_to_boxes: bool,
    /// Whether this kind's cell collision is fully determined by the block id
    /// (see [`families::collision_is_state_free`]) — a plain field so
    /// `World::collision_boxes_at` and the navigation probes can take the
    /// baked per-id table instead of a virtual resolve.
    pub collision_state_free: bool,
    /// Whether this family overrides [`ShapeSim::refine_state`] — the edit
    /// cascade's per-cell gate, a plain field so every ordinary block edit
    /// pays 7 lookups and no virtual calls when nothing shaped is nearby.
    pub refines: bool,
}

impl<'de> Deserialize<'de> for RawShape {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        // Self-describing (block rows load through serde_json): a bare string is
        // an engine family name or a namespaced custom-shape reference; an object
        // is one of the parameterized tagged forms.
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
        }
        match serde_json::from_value::<Tagged>(value).map_err(D::Error::custom)? {
            Tagged::Boxes(b) => Ok(RawShape::Boxes(b)),
            Tagged::Model(kind) => Ok(RawShape::Model(kind)),
            Tagged::Custom(custom) => Ok(RawShape::Custom(custom)),
        }
    }
}

/// Interns shape kinds during block load — one [`ShapeKindDef`] row per distinct
/// canonical key (all plain cubes share a row, a farmland and a snow lowered
/// cube are two rows, each model kind its own). The block loader interns every
/// row's resolved shape and reads back the finished table with
/// [`into_table`](Self::into_table).
pub(super) struct ShapeKindInterner {
    table: Vec<ShapeKindDef>,
    index: HashMap<String, u8>,
}

impl ShapeKindInterner {
    pub(super) fn new() -> Self {
        Self {
            table: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Intern `(family, params)` under `key`, returning the (possibly reused) id.
    pub(super) fn intern(
        &mut self,
        family: ShapeFamily,
        params: ShapeParams,
        key: String,
    ) -> Result<BlockShapeKind, String> {
        if let Some(&id) = self.index.get(&key) {
            return Ok(BlockShapeKind(id));
        }
        if self.table.len() >= 256 {
            return Err(format!(
                "too many distinct block shape kinds (256 max) registering '{key}'"
            ));
        }
        let id = self.table.len() as u8;
        let (sim, render, placement) = families::singletons(family);
        self.table.push(ShapeKindDef {
            key: Box::leak(key.clone().into_boxed_str()),
            family,
            params,
            sim,
            render,
            placement,
            resolves_to_boxes: families::resolves_to_boxes(family),
            collision_state_free: families::collision_is_state_free(family),
            refines: families::refines(family, &params),
        });
        self.index.insert(key, id);
        Ok(BlockShapeKind(id))
    }

    /// The finished id-ordered shape-kind table.
    pub(super) fn into_table(self) -> Vec<ShapeKindDef> {
        self.table
    }
}

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    pub use super::BoxDef;
    pub use super::corner_form::FACE_BEFORE_TURN;
    pub use super::FRONT_AFTER_TURN;
    pub use super::RawShape;
    pub use super::ShapeFamily;
    pub use super::ShapeParams;
    pub use crate::tile::Tile;
    pub use super::face_uv_turns;
    pub use super::facets;
    pub use super::families;
    pub use super::light_aperture_face;
    #[allow(unused_imports)]
    pub use super::*;
}
