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

use crate::block_model::BlockModelKind;
use crate::connect;

mod custom;
mod facets;
mod families;

pub use custom::{CustomLight, CustomShapeDef};
pub use facets::{ItemRender, ShapeRender, ShapeSim};

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
    LoweredCube,
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
    /// A lowered cube's visible height in texels (`1..=15`).
    LoweredCube { height: u8 },
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
    /// The lowered-cube visible height, if this is a lowered-cube kind.
    #[inline]
    pub fn lowered_height(&self) -> Option<u8> {
        match self {
            ShapeParams::LoweredCube { height } => Some(*height),
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
static ENGINE_FENCE_BOXES: [connect::Shape; 16] =
    connect::make_shapes(6.0 / 16.0, 10.0 / 16.0);
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
    LoweredCube(u8),
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
            LoweredCube(u8),
            Model(BlockModelKind),
            Custom(RawCustomShape),
        }
        match serde_json::from_value::<Tagged>(value).map_err(D::Error::custom)? {
            Tagged::LoweredCube(h) => Ok(RawShape::LoweredCube(h)),
            Tagged::Model(kind) => Ok(RawShape::Model(kind)),
            Tagged::Custom(custom) => Ok(RawShape::Custom(custom)),
        }
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
    pub(crate) fn resolve(&self) -> Result<(ShapeFamily, ShapeParams, String), String> {
        Ok(match self {
            RawShape::Cube => (ShapeFamily::Cube, ShapeParams::None, "petramond:cube".into()),
            RawShape::LoweredCube(height) => (
                ShapeFamily::LoweredCube,
                ShapeParams::LoweredCube { height: *height },
                format!("petramond:lowered_cube/{height}"),
            ),
            RawShape::Cross => (ShapeFamily::Cross, ShapeParams::None, "petramond:cross".into()),
            RawShape::Crop => (ShapeFamily::Crop, ShapeParams::None, "petramond:crop".into()),
            RawShape::Torch => (ShapeFamily::Torch, ShapeParams::None, "petramond:torch".into()),
            RawShape::Stair => (ShapeFamily::Stair, ShapeParams::None, "petramond:stair".into()),
            RawShape::Slab => (ShapeFamily::Slab, ShapeParams::None, "petramond:slab".into()),
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
            RawShape::Ladder => (ShapeFamily::Ladder, ShapeParams::None, "petramond:ladder".into()),
            RawShape::Model(kind) => (
                ShapeFamily::Model,
                ShapeParams::Model { kind: *kind },
                format!("petramond:model/{}", crate::block_model::def(*kind).key),
            ),
            RawShape::Door => (ShapeFamily::Door, ShapeParams::None, "petramond:door".into()),
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
        let (sim, render) = families::singletons(family);
        self.table.push(ShapeKindDef {
            key: Box::leak(key.clone().into_boxed_str()),
            family,
            params,
            sim,
            render,
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
        assert!(matches!(de(r#"{"lowered_cube":15}"#), RawShape::LoweredCube(15)));
        assert!(matches!(de(r#"{"custom":{"family":"fence"}}"#), RawShape::Custom(_)));
        match de(r#""mymod:gate""#) {
            RawShape::Named(key) => assert_eq!(key, "mymod:gate"),
            _ => panic!("a namespaced string is a custom-shape reference"),
        }
        // A bare (non-namespaced) unknown string is not a valid shape.
        assert!(serde_json::from_str::<RawShape>(r#""bogus""#).is_err());
    }

    fn resolve_json(s: &str) -> Result<(ShapeFamily, ShapeParams, String), String> {
        serde_json::from_str::<RawShape>(s).expect("parses").resolve()
    }

    /// The Layer-2 secondary families (`cross`/`crop`/`wall_panel`) resolve to
    /// their engine family + `Dimensions` params, texels folded to fractions.
    #[test]
    fn custom_dimension_families_resolve_to_dimension_params() {
        let (fam, params, _) =
            resolve_json(r#"{"custom":{"family":"cross","inset":4}}"#).unwrap();
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
