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
use crate::atlas::Tile;
use crate::block_model::BlockModelKind;
use crate::connect;

mod corner_form;
mod custom;
mod facets;
mod families;
mod load;
mod neighborhood;

pub use custom::{CustomLight, CustomShapeDef};
pub use facets::{
    full_face_at, light_aperture_face, pack_light_apertures, rests_flat_on_floor, FullFace,
    ItemRender, NoNeighborhood, ShapeCtx, ShapeRender, ShapeSim, LIGHT_APERTURES_OPEN,
    NO_PART_TINT,
};

#[cfg(test)]
use corner_form::FACE_BEFORE_TURN;
pub use corner_form::{face_uv_turns, FRONT_AFTER_TURN};
pub(crate) use load::{family_resolves_to_boxes, RawBox, RawCustomShape, RawShape};
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
    pub(crate) placement: &'static dyn facets::ShapePlacement,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-id collision table ([`Block::static_collision_boxes`]) is only
    /// sound while every kind flagged [`ShapeKindDef::collision_state_free`]
    /// really answers the same boxes with and without a cell to read. A family
    /// that grows a per-cell `collision_boxes` override must leave
    /// `families::collision_is_state_free` — and this is what says so.
    #[test]
    fn collision_state_free_kinds_resolve_identically() {
        use crate::block::Block;
        use crate::chunk::ChunkPos;
        use crate::world::World;
        let mut world = World::new(0, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        // Neighbours that a state-reading family WOULD react to (a fence arm, a
        // stair corner, a pane join), so a mis-flagged kind cannot pass by
        // being surrounded by air.
        world.set_block_world(8, 64, 8, Block::Stone);
        world.set_block_world(9, 65, 8, Block::Stone);
        world.set_block_world(8, 65, 9, Block::OakFence);
        for &block in Block::all() {
            let k = block.shape_kind_def();
            let Some(baked) = block.static_collision_boxes() else {
                assert!(
                    !k.collision_state_free,
                    "block id {} ({}) is flagged state-free but has no baked boxes",
                    block.id(),
                    k.key
                );
                continue;
            };
            for pos in [
                crate::mathh::IVec3::new(8, 65, 8),
                crate::mathh::IVec3::new(8, 66, 8),
                crate::mathh::IVec3::new(3, 70, 12),
            ] {
                let live = k.sim.collision_boxes(&k.params, &world, pos, block);
                assert_eq!(
                    baked,
                    live,
                    "block id {} ({}) resolves per cell at {pos:?} but is flagged state-free",
                    block.id(),
                    k.key
                );
            }
        }
    }

    /// `RawShape` accepts the engine family strings, the parameterized tagged
    /// forms, and a bare NAMESPACED string as a custom-shape reference —
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

    /// The secondary parameterized families (`cross`/`crop`/`wall_panel`) resolve to
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
