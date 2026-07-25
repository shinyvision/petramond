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
//! cache (Layer 3). The per-row payloads the old enum carried inline
//! (`LoweredCube(u8)`, `Model(kind)`) live in [`ShapeParams`], so the parameter
//! variation the Layer-2 families need is data on the row, not a code variant.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::Aabb;
use crate::atlas::Tile;
use crate::block_model::BlockModelKind;
use crate::connect;

mod custom;
mod facets;
mod families;
mod neighborhood;

pub use custom::{CustomLight, CustomShapeDef};
pub use facets::{
    full_face_at, light_aperture_face, pack_light_apertures, rests_flat_on_floor, FullFace,
    ItemRender, NoNeighborhood, ShapeCtx, ShapeRender, ShapeSim, LIGHT_APERTURES_OPEN,
    NO_PART_TINT,
};

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

    /// The canonical registry key (diagnostics + Layer 2/3 lookup).
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
/// for mod-defined procedural shapes (Layer 3). A mod never adds a variant here:
/// a Layer-2 shape reuses an existing family with different [`ShapeParams`], and
/// a Layer-3 shape is `Custom`.
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
    /// generalised Layer-2 family; the engine's only member is the ladder.
    Ladder,
    Model,
    Door,
    /// A mod-defined procedural shape, meshed/collided from the WASM bake cache
    /// (Layer 3). The [`ShapeParams::Custom`] payload carries its declaration.
    Custom,
}

/// The per-row parameters of a shape kind — what the old `RenderShape`
/// variants carried inline, plus the Layer-2 family dimensions and the Layer-3
/// custom declaration. Most engine rows are [`None`](Self::None) (the family
/// alone fully describes them).
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
    /// A mod-declared procedural shape (Layer 3): a reference to its
    /// `shapes.json` declaration. The geometry comes from the pack's WASM bake
    /// (cached per section); this carries the static metadata the engine reads
    /// without dispatching, and the fallback a trapped bake freezes to.
    Custom(&'static CustomShapeDef),
    /// A parameterized cross / crop / wall-panel (Layer 2): the numeric
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

    /// The per-cell state key a Layer-3 custom shape declares (`shapes.json`
    /// `"state_key"`), or `None` — the cell-KV key the bake input carries for
    /// the cell and its neighbours (see [`mod_api::CellInput`]).
    #[inline]
    pub fn state_key(&self) -> Option<&'static str> {
        self.custom().and_then(|c| c.state_key)
    }

    /// The custom-shape declaration, if this is a Layer-3 custom shape.
    #[inline]
    pub fn custom(&self) -> Option<&'static CustomShapeDef> {
        match self {
            ShapeParams::Custom(c) => Some(c),
            _ => None,
        }
    }

    /// The Layer-2 render/collision dimensions, if this is a parameterized
    /// cross / crop / wall-panel kind.
    #[inline]
    pub fn dimensions(&self) -> Option<&'static DimensionParams> {
        match self {
            ShapeParams::Dimensions(d) => Some(d),
            _ => None,
        }
    }
}

/// The Layer-2 render/collision dimensions of a cross / crop / wall-panel kind —
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

/// Where each face lands after ONE quarter turn about Y: entry `i` of the
/// turned box takes the authored box's face `FACE_BEFORE_TURN[i]`. The turn is
/// `(x, z) -> (1 - z, x)`, so the authored `-Z` front comes to `+X`, matching
/// [`Facing`](crate::facing::Facing)'s North → East step.
const FACE_BEFORE_TURN: [usize; 6] = [5, 4, 2, 3, 0, 1];

/// Where the AUTHORED front face (`-Z`, canonical index 5) sits after `turns`
/// quarter turns — how a row's `front` tile finds its face. The draw indexes
/// this by the face's TOTAL turn (the shape's plus the face's own
/// [`BoxDef::art_turns`]), which is what lets a corner form's front art wrap
/// around two faces that are different numbers of turns from the authored one.
///
/// The item cube needs no such lookup for the straight form: it draws at two
/// turns, which lands the front on `+Z`, exactly the index
/// `block_icon_faces_with_state` already writes the front tile to.
pub const FRONT_AFTER_TURN: [usize; 4] = [5, 0, 4, 1];

/// The [`ShapeFace::uv_turns`](crate::block::ShapeFace::uv_turns) a box face
/// needs once its shape is turned `turns` quarter turns about Y.
///
/// The four SIDE faces need none — a quarter turn carries a side face's
/// cell-local UV to the next direction unchanged. `+Y`/`-Y` sample a fixed
/// tile through a turned footprint, so their art must be turned back, and
/// opposite ways because the two mappings are mirror images. Shared by the
/// chunk mesh and the item cube so a turned shape textures the same in the
/// world, the hand and the icon.
pub fn face_uv_turns(face: usize, turns: u8) -> u8 {
    match face {
        2 => turns & 3,
        3 => (4 - (turns & 3)) & 3,
        _ => 0,
    }
}

impl BoxDef {
    /// This box turned one quarter about the cell's Y centre.
    fn turned(&self) -> BoxDef {
        let (min, max) = (self.aabb.min, self.aabb.max);
        BoxDef {
            aabb: Aabb {
                min: [1.0 - max[2], min[1], min[0]],
                max: [1.0 - min[2], max[1], max[0]],
            },
            faces: std::array::from_fn(|i| self.faces[FACE_BEFORE_TURN[i]]),
            tiles: std::array::from_fn(|i| self.tiles[FACE_BEFORE_TURN[i]]),
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            art_turns: std::array::from_fn(|i| self.art_turns[FACE_BEFORE_TURN[i]]),
        }
    }

    /// This box with every face's art frame advanced `turns` quarter turns —
    /// what a corner form's donor list needs so its inherited faces still know
    /// which frame they were authored in (see [`art_turns`](Self::art_turns)).
    fn art_advanced(&self, turns: u8) -> BoxDef {
        BoxDef {
            art_turns: self.art_turns.map(|t| (t + turns) & 3),
            ..*self
        }
    }
}

/// Turn a whole box list by `turns` quarter turns.
fn turned_list(list: &[BoxDef], turns: u8) -> Vec<BoxDef> {
    let mut v: Vec<BoxDef> = list.to_vec();
    for _ in 0..(turns & 3) {
        v = v.iter().map(BoxDef::turned).collect();
    }
    v
}

/// The quarter-turned donor list a corner form composes against: turned
/// geometry whose faces also REMEMBER they were authored one turn round.
fn donor_list(list: &[BoxDef], turns: u8) -> Vec<BoxDef> {
    turned_list(list, turns)
        .iter()
        .map(|b| b.art_advanced(turns))
        .collect()
}

/// The INTERSECTION of two box lists — the OUTER corner form, exactly the
/// stair rule's `back_mask & back_mask` lifted from quadrant masks to boxes:
/// what remains is the matter both perpendicular orientations agree on, so the
/// front treatment wraps around the turned side. Each result face inherits its
/// style from the parent whose face plane it lies on (`self` preferred where
/// both are coplanar — the top of a full-cell slab), including that parent's
/// [`art_turns`](BoxDef::art_turns), which is how the turned parent's FRONT
/// tile and UV frame reach the wrapped face.
fn intersect_lists(a: &[BoxDef], b: &[BoxDef]) -> Vec<BoxDef> {
    let mut out = Vec::new();
    for pa in a {
        for pb in b {
            let mut r = pa.aabb;
            for ax in 0..3 {
                r.min[ax] = r.min[ax].max(pb.aabb.min[ax]);
                r.max[ax] = r.max[ax].min(pb.aabb.max[ax]);
            }
            if (0..3).any(|ax| r.min[ax] >= r.max[ax]) {
                continue;
            }
            // Face i of the result lies on pa's plane, pb's plane, or strictly
            // inside one parent (then the OTHER parent's plane bounds it).
            let mut piece = BoxDef { aabb: r, ..*pa };
            for i in 0..6 {
                let (axis, high) = [
                    (0, true),
                    (0, false),
                    (1, true),
                    (1, false),
                    (2, true),
                    (2, false),
                ][i];
                let plane = if high { r.max[axis] } else { r.min[axis] };
                let of = |p: &BoxDef| {
                    if high {
                        p.aabb.max[axis] == plane
                    } else {
                        p.aabb.min[axis] == plane
                    }
                };
                let parent = if of(pa) { pa } else { pb };
                piece.faces[i] = parent.faces[i];
                piece.tiles[i] = parent.tiles[i];
                piece.art_turns[i] = parent.art_turns[i];
            }
            piece.occludes = pa.occludes && pb.occludes;
            piece.collides = pa.collides && pb.collides;
            piece.double_sided = pa.double_sided || pb.double_sided;
            if !out.contains(&piece) {
                out.push(piece);
            }
        }
    }
    out
}

/// The UNION of two box lists — the INNER corner form (`back_mask | back_mask`):
/// simply both lists, `self` first so the coincident-plane tie-break keeps the
/// straight parent's faces wherever the two overlap exactly (interpenetration
/// is the box vocabulary's normal state; buried faces are harmless overdraw).
/// Exact duplicates are dropped.
fn union_lists(a: &[BoxDef], b: &[BoxDef]) -> Vec<BoxDef> {
    let mut out: Vec<BoxDef> = a.to_vec();
    for pb in b {
        if !out.iter().any(|pa| pa.aabb == pb.aabb) {
            out.push(*pb);
        }
    }
    out
}

/// The union of every box's extent — a form's selection outline and target box.
fn union_bounds(set: &[BoxDef]) -> Aabb {
    let mut bounds = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for b in set {
        for a in 0..3 {
            bounds.min[a] = bounds.min[a].min(b.aabb.min[a]);
            bounds.max[a] = bounds.max[a].max(b.aabb.max[a]);
        }
    }
    bounds
}

/// One shape-kind registry row: the family, its canonical key, the parameters
/// that distinguish this kind from others of the same family, and the facet
/// singletons consumers dispatch through.
pub struct ShapeKindDef {
    /// Canonical key — `petramond:<family>` for a parameterless engine kind,
    /// `petramond:lowered_cube/<n>` / `petramond:model/<model_key>` for the
    /// parameterized engine kinds, or a `mod_id:name` for a Layer-2/3 kind.
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
    pub(crate) placement: &'static dyn facets::ShapePlacement,
    /// Whether this family answers [`ShapeRender::boxes`] — the mesher's
    /// per-cell gate. A plain field so the hot loop reads it without a
    /// virtual call; set from the family at intern time.
    pub resolves_to_boxes: bool,
    /// Whether this family overrides [`ShapeSim::refine_state`] — the edit
    /// cascade's per-cell gate, a plain field so every ordinary block edit
    /// pays 7 lookups and no virtual calls when nothing shaped is nearby.
    pub refines: bool,
}

/// The `shape` field of a `blocks.json` row, before resolution to a
/// [`BlockShapeKind`]. A bare family name (`"cube"`, `"stair"`, …), or an
/// externally-tagged parameterized form (`{"lowered_cube": 15}`,
/// `{"model": "petramond:bed"}`). Resolved by [`resolve`](Self::resolve) at
/// load. Layer 2 adds a `{"custom": {...}}` variant here.
/// Serialize is kept (derived) for `RawBlockDef`'s derive; deserialize is manual
/// so a bare namespaced string (`"mymod:gate"`) resolves to [`RawShape::Named`],
/// the Layer-3 custom-shape reference, alongside the enum forms.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RawShape {
    Cube,
    Boxes(Vec<RawBox>),
    Cross,
    Crop,
    Torch,
    Stair,
    Slab,
    Pane,
    Fence,
    Ladder,
    Model(BlockModelKind),
    Door,
    /// A mod-parameterized connection shape (Layer 2): `{"custom": {"family":
    /// "fence", "post_thickness": 6, …}}`.
    Custom(RawCustomShape),
    /// A Layer-3 custom shape referenced by name (`"shape": "mymod:gate"`),
    /// declared in the pack's `shapes.json`.
    Named(String),
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

/// The most boxes one authored shape may list. Shares the guest-bake cap: a
/// static shape and a WASM-baked one land in the same mesher and the same
/// per-cell budget.
const MAX_AUTHORED_BOXES: usize = crate::world::shape_bake_validate::MAX_SHAPE_BOXES;

/// Resolve `{"boxes": [...]}` to its family, leaked params, and canonical key.
/// The key spells the whole authored list (and the corners flag), so two rows
/// with identical boxes share ONE shape kind (every plain cactus is one row in
/// the table) and two that differ never collide.
fn resolve_box_set(
    raw: &[RawBox],
    corners: bool,
) -> Result<(ShapeFamily, ShapeParams, String), String> {
    if raw.is_empty() {
        return Err("a 'boxes' shape needs at least one box".into());
    }
    if raw.len() > MAX_AUTHORED_BOXES {
        return Err(format!(
            "a 'boxes' shape may list at most {MAX_AUTHORED_BOXES} boxes, got {}",
            raw.len()
        ));
    }
    let boxes: Vec<BoxDef> = raw.iter().map(RawBox::resolve).collect::<Result<_, _>>()?;
    let key = format!(
        "#boxes/{}",
        boxes
            .iter()
            .map(|b| {
                let t = |v: f32| (v * 16.0).round() as u8;
                let faces: String = b.faces.iter().map(|&f| if f { '1' } else { '0' }).collect();
                // Tiles are part of the shape's identity: two rows whose boxes
                // agree but whose face art does not are different kinds.
                let tiles: String = b
                    .tiles
                    .iter()
                    .map(|t| t.map_or(String::new(), |t| format!(".{}", t.index())))
                    .collect();
                format!(
                    "{},{},{}-{},{},{}:{faces}{}{}{}{tiles}",
                    t(b.aabb.min[0]),
                    t(b.aabb.min[1]),
                    t(b.aabb.min[2]),
                    t(b.aabb.max[0]),
                    t(b.aabb.max[1]),
                    t(b.aabb.max[2]),
                    if b.collides { "c" } else { "" },
                    if b.occludes { "o" } else { "" },
                    if b.double_sided { "d" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("/")
    ) + if corners { "+corners" } else { "" };
    // The AUTHORED-space forms, the stair rule lifted to box lists: straight,
    // outer = self INTERSECT quarter-turned self (the matter both orientations
    // agree on), inner = self UNION quarter-turned self — one clockwise and
    // one counter-clockwise of each corner. A kind that does not corner-join
    // has exactly ONE form; the five slots stay so indexing is uniform, but
    // they share one list rather than five identical copies of it.
    let authored: &'static [BoxDef] = Box::leak(boxes.clone().into_boxed_slice());
    let mut authored_forms: [&'static [BoxDef]; 5] = [authored; 5];
    if corners {
        let cw = donor_list(&boxes, 1);
        let ccw = donor_list(&boxes, 3);
        let composed = [
            intersect_lists(&boxes, &cw),
            intersect_lists(&boxes, &ccw),
            union_lists(&boxes, &cw),
            union_lists(&boxes, &ccw),
        ];
        for (slot, list) in authored_forms[1..].iter_mut().zip(composed) {
            if list.len() > MAX_AUTHORED_BOXES {
                return Err(format!(
                    "a corner form of this shape needs {} boxes (max {MAX_AUTHORED_BOXES})",
                    list.len()
                ));
            }
            *slot = Box::leak(list.into_boxed_slice());
        }
    }
    // Every (turn, form) variant is resolved HERE, at load: composed in
    // authored space, then the whole list is turned. Nothing composes or
    // rotates per cell, per frame or per collision query, and
    // `collision_boxes` can still hand out a `&'static`. Only the DISTINCT
    // forms are turned and leaked — a plain box set pays four lists, not
    // twenty identical ones.
    let mut forms: [[&'static [BoxDef]; 5]; 4] = [[&[]; 5]; 4];
    let mut collision: [[&'static [Aabb]; 5]; 4] = [[&[]; 5]; 4];
    // Every slot is written below; this is only the array's initial value.
    let mut bounds = [[Aabb {
        min: [0.0; 3],
        max: [0.0; 3],
    }; 5]; 4];
    for f in 0..if corners { 5 } else { 1 } {
        let mut set: &'static [BoxDef] = authored_forms[f];
        for (t, ((forms, collision), bounds)) in forms
            .iter_mut()
            .zip(collision.iter_mut())
            .zip(bounds.iter_mut())
            .enumerate()
        {
            if t > 0 {
                set = Box::leak(turned_list(set, 1).into_boxed_slice());
            }
            forms[f] = set;
            let c: Vec<Aabb> = set.iter().filter(|b| b.collides).map(|b| b.aabb).collect();
            collision[f] = Box::leak(c.into_boxed_slice());
            bounds[f] = union_bounds(set);
        }
    }
    if !corners {
        for t in 0..4 {
            forms[t] = [forms[t][0]; 5];
            collision[t] = [collision[t][0]; 5];
            bounds[t] = [bounds[t][0]; 5];
        }
    }
    let params: &'static BoxSetParams = Box::leak(Box::new(BoxSetParams {
        forms,
        collision,
        bounds,
        corner_joins: corners,
    }));
    Ok((ShapeFamily::BoxSet, ShapeParams::BoxSet(params), key))
}

/// Whether a family resolves to a box set — the shape-kind row's
/// [`resolves_to_boxes`](ShapeKindDef::resolves_to_boxes), reachable at LOAD
/// time (before the kind table is installed) so the loader can mirror it onto
/// the dense block flags.
pub(crate) fn family_resolves_to_boxes(family: ShapeFamily) -> bool {
    families::resolves_to_boxes(family)
}

/// One box of a `{"boxes": [...]}` shape, as authored. Extents are TEXELS
/// (`0..=16`); `from` defaults to the cell origin and `to` to the far corner,
/// so a plain full cube is `{}` and farmland is `{"to": [16, 15, 16]}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawBox {
    #[serde(default)]
    pub from: Option<[u8; 3]>,
    #[serde(default)]
    pub to: Option<[u8; 3]>,
    /// Which faces this box draws: any of `up`, `down`, `sides`, `all`, or the
    /// individual `+x`/`-x`/`+y`/`-y`/`+z`/`-z`. Absent = all six.
    #[serde(default)]
    pub faces: Option<Vec<String>>,
    /// Per-face tile overrides, keyed by the same face names as `faces`
    /// (`{"up": "mymod:shelf_top"}`). Absent faces keep the row's
    /// `[top, bottom, side]`. Naming a face the box does not draw is a load
    /// error — it is always a typo, never a no-op worth shipping.
    #[serde(default)]
    pub tiles: Option<std::collections::BTreeMap<String, String>>,
    /// Whether the box is matter — shadows and blocks light (default yes).
    #[serde(default = "yes")]
    pub occludes: bool,
    /// Whether the box obstructs movement (default yes).
    #[serde(default = "yes")]
    pub collides: bool,
    /// Draw the box's faces from both sides (default no) — for a CUTOUT face
    /// whose art must stay whole from every angle.
    #[serde(default)]
    pub double_sided: bool,
}

fn yes() -> bool {
    true
}

/// The canonical face indices a box's face name covers (`+X, -X, +Y, -Y, +Z,
/// -Z` order). One vocabulary for both `faces` and `tiles`, so a name that
/// selects faces to draw selects the same faces to texture.
fn face_group(name: &str) -> Result<&'static [usize], String> {
    Ok(match name {
        "all" => &[0, 1, 2, 3, 4, 5],
        "sides" => &[0, 1, 4, 5],
        "up" | "+y" => &[2],
        "down" | "-y" => &[3],
        "+x" => &[0],
        "-x" => &[1],
        "+z" => &[4],
        "-z" => &[5],
        other => {
            return Err(format!(
                "unknown box face '{other}' (expected all, sides, up, down, \
                 or +x/-x/+y/-y/+z/-z)"
            ))
        }
    })
}

impl RawBox {
    /// Resolve to the engine form, validating extents and face names.
    fn resolve(&self) -> Result<BoxDef, String> {
        let texel = |v: u8, name: &str| -> Result<f32, String> {
            if v > 16 {
                return Err(format!("box {name} {v} out of range (0..=16 texels)"));
            }
            Ok(v as f32 / 16.0)
        };
        let from = self.from.unwrap_or([0, 0, 0]);
        let to = self.to.unwrap_or([16, 16, 16]);
        let mut min = [0.0f32; 3];
        let mut max = [0.0f32; 3];
        for a in 0..3 {
            min[a] = texel(from[a], "from")?;
            max[a] = texel(to[a], "to")?;
            if from[a] >= to[a] {
                return Err(format!(
                    "box axis {a} is empty ({} .. {}) — 'from' must be below 'to'",
                    from[a], to[a]
                ));
            }
        }
        // Canonical face order: +X, -X, +Y, -Y, +Z, -Z.
        let mut faces = [self.faces.is_none(); 6];
        for name in self.faces.iter().flatten() {
            for &i in face_group(name)? {
                faces[i] = true;
            }
        }
        let mut tiles = [None; 6];
        for (name, tile) in self.tiles.iter().flatten() {
            let resolved =
                Tile::from_name(tile).ok_or_else(|| format!("unknown box face tile '{tile}'"))?;
            for &i in face_group(name)? {
                if !faces[i] {
                    return Err(format!(
                        "box face tile '{name}' names a face the box does not draw"
                    ));
                }
                tiles[i] = Some(resolved);
            }
        }
        Ok(BoxDef {
            aabb: Aabb { min, max },
            faces,
            tiles,
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            // Authored geometry: every face's art is in the shape's own frame.
            // Only a corner form's inherited faces ever offset this.
            art_turns: [0; 6],
        })
    }
}

/// The body of a `{"custom": {…}}` shape: a parameterized member of an existing
/// family (Layer 2 — no WASM). Dimensions are in texels (`0..=16`).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawCustomShape {
    /// The family to parameterize: `"fence"` or `"pane"`.
    pub family: String,
    /// Post thickness in texels (fence default 4, pane default 2).
    #[serde(default)]
    pub post_thickness: Option<u8>,
    /// Post low-edge offset in texels; centred when omitted.
    #[serde(default)]
    pub post_offset: Option<u8>,
    /// `"opaque_or_same_family"` | `"solid_or_same_family"` | `"same_family_only"`
    /// | `"never"`. Defaults per family (fence opaque, pane solid).
    #[serde(default)]
    pub connection_rule: Option<String>,
    /// `"segment"` | `"sprite"` | `"cube"`. Defaults per family.
    #[serde(default)]
    pub item_form: Option<String>,
    /// Cross/crop billboard-plane inset from the cell edge, texels (cross
    /// default 0 = full-cell; crop default 2).
    #[serde(default)]
    pub inset: Option<u8>,
    /// Cross plane count — the diagonal cross is two planes; only `2` is valid.
    #[serde(default)]
    pub plane_count: Option<u8>,
    /// Crop lattice vertical drop, texels (default 1).
    #[serde(default)]
    pub drop: Option<u8>,
    /// Wall-panel thickness, texels (default 1 — the ladder slab).
    #[serde(default)]
    pub thickness: Option<u8>,
    /// Wall-panel / crop visible height, texels (default 16 = full).
    #[serde(default)]
    pub height: Option<u8>,
}

impl RawShape {
    /// Resolve this raw shape to its `(family, params, canonical key)`.
    /// `corners` is the row's corner-joining flag; only a box set consumes
    /// it, so any other shape refuses it.
    pub(crate) fn resolve(
        &self,
        corners: bool,
    ) -> Result<(ShapeFamily, ShapeParams, String), String> {
        if corners && !matches!(self, RawShape::Boxes(_)) {
            return Err("'corners' requires a '{\"boxes\": [...]}' shape".into());
        }
        Ok(match self {
            RawShape::Cube => (
                ShapeFamily::Cube,
                ShapeParams::None,
                "petramond:cube".into(),
            ),
            RawShape::Boxes(raw) => resolve_box_set(raw, corners)?,
            RawShape::Cross => (
                ShapeFamily::Cross,
                ShapeParams::None,
                "petramond:cross".into(),
            ),
            RawShape::Crop => (
                ShapeFamily::Crop,
                ShapeParams::None,
                "petramond:crop".into(),
            ),
            RawShape::Torch => (
                ShapeFamily::Torch,
                ShapeParams::None,
                "petramond:torch".into(),
            ),
            RawShape::Stair => (
                ShapeFamily::Stair,
                ShapeParams::None,
                "petramond:stair".into(),
            ),
            RawShape::Slab => (
                ShapeFamily::Slab,
                ShapeParams::None,
                "petramond:slab".into(),
            ),
            RawShape::Pane => (
                ShapeFamily::Pane,
                ShapeParams::Connection(&ENGINE_PANE_PARAMS),
                "petramond:pane".into(),
            ),
            RawShape::Fence => (
                ShapeFamily::Fence,
                ShapeParams::Connection(&ENGINE_FENCE_PARAMS),
                "petramond:fence".into(),
            ),
            RawShape::Ladder => (
                ShapeFamily::Ladder,
                ShapeParams::None,
                "petramond:ladder".into(),
            ),
            RawShape::Model(kind) => (
                ShapeFamily::Model,
                ShapeParams::Model { kind: *kind },
                format!("petramond:model/{}", crate::block_model::def(*kind).key),
            ),
            RawShape::Door => (
                ShapeFamily::Door,
                ShapeParams::None,
                "petramond:door".into(),
            ),
            RawShape::Custom(c) => c.resolve()?,
            RawShape::Named(key) => {
                let def = custom::by_key(key).ok_or_else(|| {
                    format!("unknown custom shape '{key}' (declare it in the pack's shapes.json)")
                })?;
                (ShapeFamily::Custom, ShapeParams::Custom(def), key.clone())
            }
        })
    }
}

impl RawCustomShape {
    fn resolve(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        match self.family.as_str() {
            "fence" | "pane" => self.resolve_connection(),
            "cross" => self.resolve_cross(),
            "crop" => self.resolve_crop(),
            "wall_panel" => self.resolve_wall_panel(),
            other => Err(format!(
                "unknown custom shape family '{other}' \
                 (expected 'fence', 'pane', 'cross', 'crop', or 'wall_panel')"
            )),
        }
    }

    /// A texel dimension (`0..=16`) as a cell fraction, or its default.
    fn texel(&self, value: Option<u8>, default: u8, name: &str) -> Result<f32, String> {
        let v = value.unwrap_or(default);
        if v > 16 {
            return Err(format!("{name} {v} out of range (0..=16)"));
        }
        Ok(v as f32 / 16.0)
    }

    /// Error on any of the listed `(name, present)` fields that is set. Each
    /// family lists the parameters OUTSIDE its own vocabulary, so a misplaced
    /// field (a `height` on a cross, an `inset` on a wall panel) is a load error
    /// rather than a value the resolver silently drops.
    fn reject_fields(&self, fields: &[(&str, bool)]) -> Result<(), String> {
        if let Some((name, _)) = fields.iter().find(|(_, present)| *present) {
            return Err(format!("family '{}' takes no '{name}' field", self.family));
        }
        Ok(())
    }

    /// Reject the connection-only fields on a dimension family (a stray
    /// `post_thickness` or `item_form` on a crop is almost certainly a mistake).
    fn reject_connection_fields(&self) -> Result<(), String> {
        self.reject_fields(&[
            ("post_thickness", self.post_thickness.is_some()),
            ("post_offset", self.post_offset.is_some()),
            ("connection_rule", self.connection_rule.is_some()),
            ("item_form", self.item_form.is_some()),
        ])
    }

    /// Reject the dimension fields on a connection family (fence/pane take only
    /// the post/rule/item vocabulary).
    fn reject_dimension_fields(&self) -> Result<(), String> {
        self.reject_fields(&[
            ("inset", self.inset.is_some()),
            ("plane_count", self.plane_count.is_some()),
            ("drop", self.drop.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])
    }

    /// `cross`: a two-plane diagonal billboard, `inset` texels in from the edges.
    fn resolve_cross(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
        // Cross reads only `inset` + `plane_count`.
        self.reject_fields(&[
            ("drop", self.drop.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])?;
        if let Some(pc) = self.plane_count {
            if pc != 2 {
                return Err(format!("cross plane_count {pc} unsupported (only 2)"));
            }
        }
        let inset = self.texel(self.inset, 0, "inset")?;
        if inset >= 0.5 {
            return Err("cross inset must be under 8 texels".into());
        }
        let params = Box::leak(Box::new(DimensionParams {
            inset,
            drop: 0.0,
            thickness: 0.0,
            height: 1.0,
        }));
        let key = format!("#custom/cross/inset{}", self.inset.unwrap_or(0));
        Ok((ShapeFamily::Cross, ShapeParams::Dimensions(params), key))
    }

    /// `crop`: a four-plane lattice, `inset` in from the edges and `drop` texels
    /// toward the floor (the engine crop is inset 2, drop 1).
    fn resolve_crop(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
        // Crop reads only `inset` + `drop`.
        self.reject_fields(&[
            ("plane_count", self.plane_count.is_some()),
            ("thickness", self.thickness.is_some()),
            ("height", self.height.is_some()),
        ])?;
        let inset = self.texel(self.inset, 2, "inset")?;
        let drop = self.texel(self.drop, 1, "drop")?;
        if inset >= 0.5 {
            return Err("crop inset must be under 8 texels".into());
        }
        let params = Box::leak(Box::new(DimensionParams {
            inset,
            drop,
            thickness: 0.0,
            height: 1.0,
        }));
        let key = format!(
            "#custom/crop/inset{}/drop{}",
            self.inset.unwrap_or(2),
            self.drop.unwrap_or(1)
        );
        Ok((ShapeFamily::Crop, ShapeParams::Dimensions(params), key))
    }

    /// `wall_panel`: the ladder family with a retuned slab `thickness` and
    /// `height` (the engine ladder is thickness 1, height 16). Facing is per-cell
    /// block state, as for the ladder.
    fn resolve_wall_panel(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_connection_fields()?;
        // Wall panel reads only `thickness` + `height`.
        self.reject_fields(&[
            ("inset", self.inset.is_some()),
            ("plane_count", self.plane_count.is_some()),
            ("drop", self.drop.is_some()),
        ])?;
        let thickness = self.texel(self.thickness, 1, "thickness")?;
        let height = self.texel(self.height, 16, "height")?;
        if thickness == 0.0 {
            return Err("wall_panel thickness must be at least 1 texel".into());
        }
        if height == 0.0 {
            return Err("wall_panel height must be at least 1 texel".into());
        }
        let params = Box::leak(Box::new(DimensionParams {
            inset: 0.0,
            drop: 0.0,
            thickness,
            height,
        }));
        let key = format!(
            "#custom/wall_panel/th{}/h{}",
            self.thickness.unwrap_or(1),
            self.height.unwrap_or(16)
        );
        Ok((ShapeFamily::Ladder, ShapeParams::Dimensions(params), key))
    }

    /// `fence` / `pane`: the parameterized connection families.
    fn resolve_connection(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        self.reject_dimension_fields()?;
        let family = match self.family.as_str() {
            "fence" => ShapeFamily::Fence,
            "pane" => ShapeFamily::Pane,
            other => {
                return Err(format!(
                    "unknown custom shape family '{other}' (expected 'fence' or 'pane')"
                ))
            }
        };
        let default_thickness = if family == ShapeFamily::Fence { 4 } else { 2 };
        let thickness = self.post_thickness.unwrap_or(default_thickness);
        if !(1..=16).contains(&thickness) {
            return Err(format!("post_thickness {thickness} out of range (1..=16)"));
        }
        let offset = self.post_offset.unwrap_or((16 - thickness) / 2);
        if offset as u16 + thickness as u16 > 16 {
            return Err(format!(
                "post_offset {offset} + post_thickness {thickness} exceeds 16"
            ));
        }
        let post_lo = offset as f32 / 16.0;
        let post_hi = (offset + thickness) as f32 / 16.0;
        let rule = match self.connection_rule.as_deref() {
            None if family == ShapeFamily::Fence => ConnectionRule::OpaqueOrSame,
            None => ConnectionRule::SolidOrSame,
            Some("opaque_or_same_family") => ConnectionRule::OpaqueOrSame,
            Some("solid_or_same_family") => ConnectionRule::SolidOrSame,
            Some("same_family_only") => ConnectionRule::SameOnly,
            Some("never") => ConnectionRule::Never,
            Some(other) => return Err(format!("unknown connection_rule '{other}'")),
        };
        let item_form = match self.item_form.as_deref() {
            None if family == ShapeFamily::Fence => ItemForm::Segment,
            None => ItemForm::Sprite,
            Some("segment") => ItemForm::Segment,
            Some("sprite") => ItemForm::Sprite,
            Some("cube") => ItemForm::Cube,
            Some(other) => return Err(format!("unknown item_form '{other}'")),
        };
        // Only the fence family builds a no-neighbour item segment (posts +
        // rails); a pane/bar with `item_form: "segment"` has no such geometry.
        if item_form == ItemForm::Segment && family != ShapeFamily::Fence {
            return Err("item_form 'segment' requires the 'fence' family".into());
        }
        // A mod's custom shape leaks its box table + params once (deduped by the
        // interner key, so identical customs share one).
        let boxes: &'static [connect::Shape; 16] =
            Box::leak(Box::new(connect::make_shapes(post_lo, post_hi)));
        let params: &'static ConnectionParams = Box::leak(Box::new(ConnectionParams {
            post_lo,
            post_hi,
            rule,
            item_form,
            boxes,
        }));
        let key = format!(
            "#custom/{}/off{offset}/th{thickness}/{rule:?}/{item_form:?}",
            self.family
        );
        Ok((family, ShapeParams::Connection(params), key))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `RawShape` accepts the engine family strings, the parameterized tagged
    /// forms, and a bare NAMESPACED string as a Layer-3 custom-shape reference —
    /// while a bare unknown (non-namespaced) string is a load error.
    #[test]
    fn raw_shape_deserializes_families_params_and_named_references() {
        let de = |s: &str| serde_json::from_str::<RawShape>(s).expect("parses");
        assert!(matches!(de(r#""cube""#), RawShape::Cube));
        assert!(matches!(de(r#""fence""#), RawShape::Fence));
        assert!(matches!(de(r#""door""#), RawShape::Door));
        // A box list: extents default to the whole cell, faces to all six,
        // and a box may declare that it draws without obstructing.
        let (family, params, _) = resolve_json(
            r#"{"boxes":[{"to":[16,15,16]},{"from":[0,15,0],"faces":["up"],"collides":false}]}"#,
        )
        .unwrap();
        assert_eq!(family, ShapeFamily::BoxSet);
        let set = params.box_set().expect("box set params");
        assert_eq!(set.boxes(0, 0).len(), 2);
        assert_eq!(set.boxes(0, 0)[0].aabb.max, [1.0, 15.0 / 16.0, 1.0]);
        assert_eq!(set.boxes(0, 0)[0].faces, [true; 6]);
        assert!(set.boxes(0, 0)[0].collides);
        // Face order is +X, -X, +Y, -Y, +Z, -Z.
        assert_eq!(
            set.boxes(0, 0)[1].faces,
            [false, false, true, false, false, false]
        );
        // Only the colliding box is collision; the outline is the drawn union.
        assert_eq!(set.collision(0, 0).len(), 1);
        assert_eq!(set.bounds(0, 0).max, [1.0, 1.0, 1.0]);
        // Empty lists, inverted extents, out-of-range texels, unknown face
        // names and a tile on an undrawn face are load errors, not silently
        // dropped values.
        for bad in [
            r#"{"boxes":[]}"#,
            r#"{"boxes":[{"from":[8,0,0],"to":[8,16,16]}]}"#,
            r#"{"boxes":[{"to":[17,16,16]}]}"#,
            r#"{"boxes":[{"faces":["sideways"]}]}"#,
            r#"{"boxes":[{"tiles":{"up":"petramond:no_such_tile"}}]}"#,
            r#"{"boxes":[{"faces":["up"],"tiles":{"down":"stone"}}]}"#,
        ] {
            assert!(resolve_json(bad).is_err(), "{bad}");
        }
        assert!(matches!(
            de(r#"{"custom":{"family":"fence"}}"#),
            RawShape::Custom(_)
        ));
        match de(r#""mymod:gate""#) {
            RawShape::Named(key) => assert_eq!(key, "mymod:gate"),
            _ => panic!("a namespaced string is a custom-shape reference"),
        }
        // A bare (non-namespaced) unknown string is not a valid shape.
        assert!(serde_json::from_str::<RawShape>(r#""bogus""#).is_err());
    }

    /// Apertures ask whether matter SEALS a boundary, not whether it fills the
    /// octant behind it. A cover that stops a texel short of its top must read
    /// OPEN above — otherwise its own cell floods to black, the sunken top the
    /// mesher draws inside that cell samples the dark, and every neighbouring
    /// face averages its smooth light against a cell that is plainly lit
    /// (the 2026-07-25 black-farmland playtest bug).
    #[test]
    fn a_cover_that_stops_short_of_its_top_stays_open_to_the_light() {
        let apertures = |json: &str| {
            let (family, params, _) = resolve_json(json).unwrap();
            let (sim, ..) = families::singletons(family);
            sim.light_apertures(
                &params,
                &facets::NoNeighborhood,
                crate::mathh::IVec3::ZERO,
                crate::block::Block::Air,
            )
        };
        // 15/16 tall: fills most of its top octant, seals none of its top face.
        let farmland = apertures(r#"{"boxes":[{"to":[16,15,16]}]}"#);
        assert_eq!(
            light_aperture_face(farmland, (0, 1, 0)),
            0b1111,
            "an unsealed top must let light in"
        );
        assert_eq!(
            light_aperture_face(farmland, (0, -1, 0)),
            0,
            "its floor-flush base still seals downward"
        );
        assert_eq!(
            light_aperture_face(farmland, (1, 0, 0)),
            0,
            "its sides reach the boundary on both halves"
        );
        // An inset column under an overhanging cap: sealed top and bottom, but
        // its SIDES stay open. The cap clips the extreme texel of every side
        // quadrant, so a probe over the whole quadrant would call the cell
        // sealed and black it out — the cactus half of the same playtest bug.
        let capped = apertures(
            r#"{"boxes":[{"from":[1,0,1],"to":[15,16,15]},{"from":[0,15,0],"faces":["up"]}]}"#,
        );
        assert_eq!(light_aperture_face(capped, (0, 1, 0)), 0, "the cap seals");
        assert_eq!(
            light_aperture_face(capped, (0, -1, 0)),
            0,
            "the trunk seals its own floor"
        );
        assert_eq!(
            light_aperture_face(capped, (1, 0, 0)),
            0b1111,
            "an inset trunk leaves its recessed sides open to the light"
        );
    }

    fn resolve_json(s: &str) -> Result<(ShapeFamily, ShapeParams, String), String> {
        serde_json::from_str::<RawShape>(s)
            .expect("parses")
            .resolve(false)
    }

    /// [`resolve_json`] with the row's corner-joining flag set.
    fn resolve_corners(s: &str) -> Result<(ShapeFamily, ShapeParams, String), String> {
        serde_json::from_str::<RawShape>(s)
            .expect("parses")
            .resolve(true)
    }

    /// Turning a box set is a quarter turn about Y — an order-4 action, so
    /// four turns must land back on the authored list, geometry AND per-face
    /// data together. This is what catches a face permutation that disagrees
    /// with the extent swap: the individual turns still "look" plausible, but
    /// a shape's front art walks off its front.
    #[test]
    fn four_quarter_turns_return_a_box_set_to_its_authored_form() {
        // Deliberately asymmetric on every axis and per face, so a wrong
        // permutation cannot coincide with the right one.
        let (_, params, _) = resolve_json(
            r#"{"boxes":[
                 {"from":[1,2,3],"to":[5,14,7],"faces":["+x","up","-z"],
                  "tiles":{"up":"stone","-z":"dirt"}},
                 {"from":[0,0,9],"to":[16,1,16],"faces":["all"],"tiles":{"+x":"sand"}}
               ]}"#,
        )
        .unwrap();
        let set = params.box_set().expect("box set params");
        let four: Vec<BoxDef> = set.boxes(3, 0).iter().map(BoxDef::turned).collect();
        assert_eq!(four, set.boxes(0, 0), "four quarter turns is the identity");
        // ...and no intermediate turn is: an authored front must actually move.
        for t in 1..4 {
            assert_ne!(set.boxes(0, 0), set.boxes(t, 0), "turn {t} must differ");
        }
        // One turn carries the authored -Z front to +X, matching Facing's
        // North -> East step (the convention `FRONT_AFTER_TURN` encodes).
        assert!(set.boxes(0, 0)[0].faces[5] && set.boxes(1, 0)[0].faces[FRONT_AFTER_TURN[1]]);
        assert_eq!(
            set.boxes(0, 0)[0].tiles[5],
            set.boxes(1, 0)[0].tiles[FRONT_AFTER_TURN[1]],
            "the front TILE travels with the front face"
        );
        // The collision and outline views are the same turn, not a stale
        // authored copy.
        for t in 0..4u8 {
            let boxes = set.boxes(t, 0);
            let collision: Vec<_> = boxes
                .iter()
                .filter(|b| b.collides)
                .map(|b| b.aabb)
                .collect();
            assert_eq!(set.collision(t, 0), collision, "turn {t} collision");
            for b in boxes {
                for a in 0..3 {
                    assert!(set.bounds(t, 0).min[a] <= b.aabb.min[a], "turn {t} bounds");
                    assert!(set.bounds(t, 0).max[a] >= b.aabb.max[a], "turn {t} bounds");
                }
            }
        }
    }

    /// The UV turn must exactly undo what turning the shape did to a face's
    /// cell-local UV: sampling a turned box at the turned point has to land on
    /// the same texel as sampling the authored box at the authored point, or a
    /// tile authored once cannot serve all four facings.
    ///
    /// The sides come out right for free; `+Y`/`-Y` are the two that need the
    /// correction, in OPPOSITE directions, which is exactly the pair a
    /// hand-derived sign gets backwards.
    #[test]
    fn the_uv_turn_undoes_the_shape_turn_on_every_face() {
        use crate::mesh::face::Face;
        const FACES: [Face; 6] = Face::ALL;
        use crate::mesh::plane::cell_uv;

        // A cell-local point off-centre on every axis, so no symmetry can hide
        // a mistake.
        let authored = [3.0 / 16.0, 5.0 / 16.0, 6.0 / 16.0];
        for turns in 0..4u8 {
            // The same material point after `turns` quarter turns: the turn the
            // box extents get, (x, z) -> (1 - z, x).
            let mut p = authored;
            for _ in 0..turns {
                p = [1.0 - p[2], p[1], p[0]];
            }
            for (i, face) in FACES.into_iter().enumerate() {
                // Face `i` of the turned box is authored face `a`; sampling the
                // turned face at the turned point must land where the authored
                // face sampled the authored point.
                let want = cell_uv(FACES[FACE_BEFORE_TURN_N(i, turns)], authored);
                let [u, v] = cell_uv(face, p);
                let got = crate::block::ShapeFace::turn_uv(face_uv_turns(i, turns), u, v);
                assert!(
                    (got.0 - want[0]).abs() < 1e-5 && (got.1 - want[1]).abs() < 1e-5,
                    "turn {turns} face {i}: sampled {got:?}, authored {want:?}"
                );
            }
        }
    }

    /// Which authored face ends up at canonical index `i` after `turns`.
    #[allow(non_snake_case)]
    fn FACE_BEFORE_TURN_N(i: usize, turns: u8) -> usize {
        (0..turns).fold(i, |f, _| FACE_BEFORE_TURN[f])
    }

    /// Whether face `i` of `b` draws the row's `front` tile once the shape is
    /// turned `turns` — the exact predicate `families::box_set_box` applies,
    /// restated here so these tests pin the BEHAVIOUR rather than the field it
    /// happens to be derived from.
    fn draws_front(b: &BoxDef, i: usize, turns: u8) -> bool {
        i == FRONT_AFTER_TURN[((turns + b.art_turns[i]) & 3) as usize]
    }

    /// The corner forms are the stair rule lifted from quadrant masks to box
    /// lists: OUTER = the shape intersected with its quarter-turned self (the
    /// matter both perpendicular orientations agree on), INNER = the union.
    /// Straight, lone, and end-of-run cells keep the AUTHORED geometry
    /// untouched — corner joining must never change a shape's resting look
    /// (the 2026-07-25 inset misdesign changed every isolated unit and is
    /// exactly what this pins against).
    #[test]
    fn corner_forms_are_the_turned_intersection_and_union_of_the_shape() {
        // A counter: full-cell top slab over a body whose front (`-Z`) is
        // inset 2 texels.
        let (_, params, key) = resolve_corners(
            r#"{"boxes":[
                 {"from":[0,14,0],"to":[16,16,16]},
                 {"from":[0,0,2],"to":[16,14,16]}
               ]}"#,
        )
        .unwrap();
        let set = params.box_set().expect("box set params");
        assert!(set.corner_joins);
        assert!(key.ends_with("+corners"), "the flag is kind identity");
        let t = |v: i32| v as f32 / 16.0;
        // STRAIGHT is byte-identical to the authored list.
        let straight = set.boxes(0, 0);
        assert_eq!(straight.len(), 2);
        assert_eq!(straight[1].aabb.min, [0.0, 0.0, t(2)]);
        assert_eq!(straight[1].aabb.max, [1.0, t(14), 1.0]);
        // OUTER: the body keeps only what a quarter-turned body also covers,
        // so the front inset wraps around the turned side; the full-cell top
        // stays whole. Form 1 = the perpendicular neighbour one turn
        // clockwise (its front toward `+X` -> its body ends at x=14).
        let outer = set.boxes(0, 1);
        let body: Vec<_> = outer.iter().filter(|b| b.aabb.max[1] < 1.0).collect();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].aabb.min, [0.0, 0.0, t(2)]);
        assert_eq!(body[0].aabb.max, [t(14), t(14), 1.0]);
        // ...and the wrapped face inherits the turned parent's authoring
        // FRAME, so the row's `front` tile lands on it too and the apron art
        // continues around the corner. The draw asks exactly this question.
        assert!(draws_front(body[0], 5, 0), "authored front still front");
        assert!(
            draws_front(body[0], 0, 0),
            "wrapped +X face draws front art"
        );
        assert!(!draws_front(body[0], 1, 0), "back-side face stays side art");
        // ...and that is what the DRAW puts on the face: TWO faces of one box
        // carry the row's `front` tile, which no single turn index can name.
        let furnace = crate::block::Block::Furnace;
        let front = furnace.front_tile().expect("the furnace row has a front");
        let drawn = families::box_set_box(body[0], 0, furnace, &|_| [1.0; 3]);
        let tile_at = |i: usize| drawn.faces[i].expect("a drawn face").tile;
        assert_eq!(tile_at(5), front, "authored front");
        assert_eq!(tile_at(0), front, "wrapped corner front");
        assert_eq!(
            tile_at(1),
            furnace.tiles()[2],
            "the far side stays side art"
        );
        // INNER: the union — both bodies, straight parent first (coincident
        // tie-break), duplicates (the identical top) dropped.
        let inner = set.boxes(0, 3);
        assert_eq!(inner.len(), 3, "top + both bodies");
        assert_eq!(inner[1].aabb.max, [1.0, t(14), 1.0]);
        assert_eq!(inner[2].aabb.max, [t(14), t(14), 1.0]);
        // Turning distributes over the composition: form F at turn t is
        // turn^t of form F at turn 0.
        for form in 0..5u8 {
            let expect: Vec<_> = set.boxes(0, form).iter().map(|b| b.turned()).collect();
            assert_eq!(set.boxes(1, form), &expect[..], "form {form} turns whole");
        }
        // Collision follows the same variant; the outline never shrinks (the
        // top spans the cell in every form).
        assert_eq!(set.collision(0, 1).len(), 2);
        assert_eq!(set.bounds(0, 1).max, [1.0; 3]);
        // A stale stored byte past the vocabulary reads as STRAIGHT, never a
        // panic or a garbage index (old worlds hold old bytes until the load
        // sweep rewrites them).
        assert_eq!(set.boxes(0, 9), set.boxes(0, 0));
        // A plain box set: one form, no refinement, indexing still uniform —
        // and the five slots SHARE one leaked list rather than holding five
        // identical copies of it.
        let (_, plain, _) = resolve_json(r#"{"boxes":[{"to":[16,15,16]}]}"#).unwrap();
        let plain = plain.box_set().unwrap();
        assert!(!plain.corner_joins);
        for turns in 0..4u8 {
            for form in 0..5 {
                assert!(
                    std::ptr::eq(plain.boxes(turns, form), plain.boxes(turns, 0)),
                    "a formless kind must not leak a copy per form slot"
                );
                assert!(std::ptr::eq(
                    plain.collision(turns, form),
                    plain.collision(turns, 0)
                ));
            }
        }
        for b in plain.boxes(0, 0) {
            assert_eq!(b.art_turns, [0; 6], "authored art is in its own frame");
        }
        // The flag off a boxes shape is a load error.
        assert!(
            serde_json::from_str::<RawShape>(r#""cube""#)
                .unwrap()
                .resolve(true)
                .is_err(),
            "'corners' requires a boxes shape"
        );
    }

    /// A corner form's inherited face carries its PARENT's authoring frame,
    /// and every frame-dependent decision must read that frame rather than the
    /// cell's turn alone.
    ///
    /// The wrapped FRONT is covered above; this pins the other half, the `±Y`
    /// UV counter-rotation. It needs a shape whose intersection is bounded by
    /// the TURNED parent's top — the counter's two boxes are both full-cell or
    /// both coplanar there, so they never expose it. With `face_uv_turns` read
    /// off the cell's turn alone, this piece's inherited top tile draws a
    /// quarter turn off, invisibly for symmetric art and wrongly for anything
    /// else.
    #[test]
    fn an_inherited_top_face_is_uv_turned_by_its_parents_frame() {
        // A low full-cell shelf with its own `up` art, under a tall half-depth
        // riser. Turning the shelf is the identity; turning the riser is not.
        let (_, params, _) = resolve_corners(
            r#"{"boxes":[
                 {"from":[0,0,0],"to":[16,6,16],"tiles":{"up":"stone"}},
                 {"from":[0,0,0],"to":[16,16,8]}
               ]}"#,
        )
        .unwrap();
        let set = params.box_set().expect("box set params");
        let t = |v: i32| v as f32 / 16.0;
        // riser ∩ turn(shelf): the shelf's top bounds it, so its `+Y` face —
        // tile and frame — comes from the TURNED shelf.
        let outer = set.boxes(0, 1);
        let piece = outer
            .iter()
            .find(|b| b.aabb.max == [1.0, t(6), t(8)])
            .expect("riser clipped by the turned shelf");
        assert_eq!(
            piece.tiles[2],
            Tile::from_name("stone"),
            "inherited up tile"
        );
        assert_eq!(piece.art_turns[2], 1, "...authored one turn round");
        // A face bounded by the shape's OWN box keeps frame 0 throughout.
        let own = outer
            .iter()
            .find(|b| b.aabb.max == [1.0, t(6), 1.0])
            .expect("shelf ∩ turned shelf");
        assert_eq!(own.art_turns, [0; 6]);
        // What actually matters is the DRAW: two tops of the same form, same
        // tile, in the same cell, must be counter-rotated DIFFERENTLY because
        // they were authored in different frames. Reading the cell's turn
        // alone gives both `0` and is the bug this pins.
        let drawn_top = |b: &BoxDef| {
            families::box_set_box(b, 0, crate::block::Block::Stone, &|_| [1.0; 3]).faces[2]
                .expect("a top face")
                .uv_turns
        };
        assert_eq!(drawn_top(piece), 1, "inherited top turns with its parent");
        assert_eq!(drawn_top(own), 0, "the shape's own top does not");
        // The frame is a RELATIVE offset, so turning the whole form carries it
        // to the face it followed and never changes its value.
        let turned = set.boxes(1, 1);
        let moved = turned
            .iter()
            .find(|b| b.aabb.min == [t(8), 0.0, 0.0] && b.aabb.max == [1.0, t(6), 1.0])
            .expect("the same piece, one turn on");
        assert_eq!(moved.art_turns[2], 1);
        assert_eq!(moved.tiles[2], Tile::from_name("stone"));
    }

    /// The Layer-2 secondary families (`cross`/`crop`/`wall_panel`) resolve to
    /// their engine family + `Dimensions` params, texels folded to fractions.
    #[test]
    fn custom_dimension_families_resolve_to_dimension_params() {
        let (fam, params, _) = resolve_json(r#"{"custom":{"family":"cross","inset":4}}"#).unwrap();
        assert_eq!(fam, ShapeFamily::Cross);
        assert_eq!(params.dimensions().unwrap().inset, 4.0 / 16.0);

        let (fam, params, _) =
            resolve_json(r#"{"custom":{"family":"crop","inset":3,"drop":2}}"#).unwrap();
        assert_eq!(fam, ShapeFamily::Crop);
        let d = params.dimensions().unwrap();
        assert_eq!((d.inset, d.drop), (3.0 / 16.0, 2.0 / 16.0));

        // A wall_panel is the ladder family with a retuned slab.
        let (fam, params, _) =
            resolve_json(r#"{"custom":{"family":"wall_panel","thickness":4,"height":12}}"#)
                .unwrap();
        assert_eq!(fam, ShapeFamily::Ladder);
        let d = params.dimensions().unwrap();
        assert_eq!((d.thickness, d.height), (4.0 / 16.0, 12.0 / 16.0));

        // Omitted dims fall back to the engine defaults (crop inset 2 / drop 1).
        let (_, params, _) = resolve_json(r#"{"custom":{"family":"crop"}}"#).unwrap();
        let d = params.dimensions().unwrap();
        assert_eq!((d.inset, d.drop), (2.0 / 16.0, 1.0 / 16.0));
    }

    /// Load-time validation rejects out-of-range dims, unknown families, a
    /// nonsense cross plane count, and connection fields on a dimension family.
    #[test]
    fn custom_dimension_families_validate() {
        assert!(resolve_json(r#"{"custom":{"family":"cross","inset":8}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"crop","inset":20}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"wall_panel","thickness":0}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"cross","plane_count":3}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"pyramid"}}"#).is_err());
        // A connection field on a crop is almost certainly a mistake.
        assert!(resolve_json(r#"{"custom":{"family":"crop","post_thickness":4}}"#).is_err());
    }
}
