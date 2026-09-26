//! Animated block models: the moving geometry a block row draws OUTSIDE the
//! chunk mesh — a chest's hinged lid, a door's swing, a trapdoor's panel —
//! declared as data in the layered `animated_models.json` catalog.
//!
//! A model is a list of boxes ("parts") per VARIANT, each face textured by a
//! named atlas tile or one of the owning row's `[top, bottom, side]` slots
//! (`"$top"`, `"$bottom"`, `"$side"`), optionally swung about a JOINT by the
//! model's open fraction. A block row names its model (`"animated_model":
//! "petramond:door"`); its shape family says how a cell poses it
//! ([`ShapeRender::animated_pose`](crate::block::ShapeRender): facing,
//! variant, open state). Everything downstream — the client gather, the
//! eased open fraction, the renderer's one bake and cull loop — is generic
//! over this, so a pack adds a barrel, a gate or a lectern as rows here plus
//! a block row, with no engine edit.
//!
//! Every model is authored in the canonical frame of the engine's dynamic
//! blocks: one unit cell (texels `0..16`, parts may reach a cell beyond),
//! front / closed edge on `+Z` (south); the renderer turns it to the pose's
//! facing about the cell's vertical centre.
//!
//! Ids are session-local (nothing persists or ships them): rows reference
//! models by key, resolved once at block load.

use std::collections::HashMap;

use serde::Deserialize;

use crate::block::{Aabb, Block};
use crate::facing::Facing;
use crate::tile::Tile;

/// Engine model keys in frozen id order.
const ENGINE_MODEL_NAMES: &[&str] = &["petramond:chest", "petramond:door", "petramond:trapdoor"];

/// How far (cell fraction) the sampled swing bounds are padded, covering the
/// arc a part's corner bulges past the chords between samples.
const CULL_MARGIN: f32 = 1.0 / 64.0;
/// Swing samples (plus the closed pose) the cull bounds are taken over.
const CULL_SAMPLES: usize = 16;

/// How one cell of an animated block is posed — what its shape family reads
/// out of the cell's own state (see
/// [`ShapeRender::animated_pose`](crate::block::ShapeRender)).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AnimatedPose {
    /// The direction the model's canonical `+Z` front is turned to.
    pub facing: Facing,
    /// Which of the model's variants this cell draws (a trapdoor's floor or
    /// ceiling panel). An out-of-range index draws variant `0`.
    pub variant: u8,
    /// Whether the cell's state says it stands open (a door's toggle). A
    /// container's lid is opened by whoever looks inside instead, which the
    /// client folds in on top.
    pub open: bool,
}

/// Where a face's tile comes from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FaceTile {
    /// One of the owning row's `[top, bottom, side]` tiles, by index — so
    /// every wood's door shares one model.
    Row(usize),
    /// A fixed atlas tile.
    Tile(Tile),
}

/// The hinge a part swings about as the model opens.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Joint {
    /// The rotation axis (`0` = X, `1` = Y, `2` = Z), right-handed.
    pub axis: usize,
    /// A point on the hinge line, cell-local (`0..1`).
    pub pivot: [f32; 3],
    /// The swing at fully open, radians.
    pub open_radians: f32,
}

impl Joint {
    /// The swing angle `open01` of the way open.
    #[inline]
    pub fn angle(&self, open01: f32) -> f32 {
        open01.clamp(0.0, 1.0) * self.open_radians
    }

    /// Rotate cell-local point `p` about the hinge by the angle whose sine
    /// and cosine are `(s, c)`.
    #[inline]
    pub fn rotate(&self, p: [f32; 3], (s, c): (f32, f32)) -> [f32; 3] {
        let d = [
            p[0] - self.pivot[0],
            p[1] - self.pivot[1],
            p[2] - self.pivot[2],
        ];
        let h = self.pivot;
        match self.axis {
            0 => [p[0], h[1] + d[1] * c - d[2] * s, h[2] + d[1] * s + d[2] * c],
            1 => [h[0] + d[0] * c + d[2] * s, p[1], h[2] - d[0] * s + d[2] * c],
            _ => [h[0] + d[0] * c - d[1] * s, h[1] + d[0] * s + d[1] * c, p[2]],
        }
    }
}

/// One box of a model variant.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelPart {
    /// Cell-local extent in the canonical (closed, south-facing) pose.
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// Per-face tiles in canonical face order (`+X, -X, +Y, -Y, +Z, -Z`).
    pub faces: [FaceTile; 6],
    /// Faces whose art is mirrored horizontally — a panel's back reads with
    /// the same handedness as its front.
    pub mirror: [bool; 6],
    /// Whether the box's panel-thin edge faces crop their tile to a matching
    /// strip instead of squishing the whole tile across the edge.
    pub thin_edges: bool,
    /// The hinge this part swings about, or `None` for a part that stays put.
    pub joint: Option<Joint>,
}

impl ModelPart {
    /// This part's per-face tiles for a cell of `block`.
    pub fn tiles(&self, block: Block) -> [Tile; 6] {
        let row = block.tiles();
        self.faces.map(|f| match f {
            FaceTile::Row(slot) => row[slot],
            FaceTile::Tile(tile) => tile,
        })
    }

    /// The eight corners of the box.
    fn corners(&self) -> impl Iterator<Item = [f32; 3]> + '_ {
        (0..8).map(|i| {
            [
                if i & 1 == 0 { self.min[0] } else { self.max[0] },
                if i & 2 == 0 { self.min[1] } else { self.max[1] },
                if i & 4 == 0 { self.min[2] } else { self.max[2] },
            ]
        })
    }
}

/// One variant of a model: its parts, plus the bounds its swing sweeps.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelVariant {
    pub parts: &'static [ModelPart],
    /// Cell-local bounds of every pose between closed and fully open,
    /// symmetric about the cell's vertical centre line so they hold under
    /// any facing — the renderer's view-cull box.
    pub cull: Aabb,
}

/// One registered animated model.
#[derive(Debug, PartialEq)]
pub struct AnimatedModelDef {
    pub key: &'static str,
    /// How fast the open fraction eases toward its target, per second.
    pub open_speed: f32,
    /// Whether the block's ITEM (icon, in hand, dropped) draws this model,
    /// closed, instead of its shape's item form.
    pub item: bool,
    /// How far the item form is lifted so the closed variant `0` sits
    /// vertically centred in the item's unit cube.
    pub item_lift: f32,
    variants: &'static [ModelVariant],
}

impl AnimatedModelDef {
    /// Variant `index`, or variant `0` for an index the model does not have.
    #[inline]
    pub fn variant(&self, index: u8) -> &'static ModelVariant {
        self.variants
            .get(index as usize)
            .unwrap_or(&self.variants[0])
    }

    /// Every variant, in index order.
    #[inline]
    pub fn variants(&self) -> &'static [ModelVariant] {
        self.variants
    }
}

/// The model registered under `key`, or `None` when no such row is loaded.
pub fn by_key(key: &str) -> Option<&'static AnimatedModelDef> {
    catalog().id(key).map(|id| &catalog().rows()[id as usize])
}

/// The animated-model catalog stage of every content registry (parts name
/// tiles, so it builds after the tiles stage).
pub(crate) static CATALOG: crate::content::Slot<crate::registry::Catalog<AnimatedModelDef>> =
    crate::content::Slot::new(
        crate::content::stage::ANIMATED_MODELS,
        &[crate::content::stage::TILES],
        load,
    );

fn load(
    reg: &crate::content::ContentRegistry,
) -> Result<crate::registry::Catalog<AnimatedModelDef>, String> {
    crate::registry::read_catalog(
        reg.packs(),
        "animated_models.json",
        "animated model",
        parse_layers,
    )
}

fn catalog() -> &'static crate::registry::Catalog<AnimatedModelDef> {
    CATALOG.current()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    model: String,
    open_speed: f32,
    #[serde(default)]
    item: bool,
    variants: Vec<RawVariant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVariant {
    parts: Vec<RawPart>,
}

/// One part as authored. Extents and pivots are TEXELS (`16` per cell).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPart {
    from: [f32; 3],
    to: [f32; 3],
    /// Face name (`east`, `west`, `up`, `down`, `south`, `north`) → tile; an
    /// `all` entry covers every face not named.
    faces: HashMap<String, String>,
    #[serde(default)]
    mirror: Vec<String>,
    #[serde(default)]
    thin_edges: bool,
    #[serde(default)]
    joint: Option<RawJoint>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJoint {
    axis: String,
    pivot: [f32; 3],
    open_degrees: f32,
}

#[derive(Deserialize)]
struct RawFile {
    animated_models: Vec<RawModel>,
}

/// Canonical face order (`+X, -X, +Y, -Y, +Z, -Z`) by authored name.
const FACE_NAMES: [&str; 6] = ["east", "west", "up", "down", "south", "north"];

fn face_index(name: &str) -> Result<usize, String> {
    FACE_NAMES
        .iter()
        .position(|&n| n == name)
        .ok_or_else(|| format!("unknown face '{name}' (expected one of {FACE_NAMES:?})"))
}

fn face_tile(name: &str) -> Result<FaceTile, String> {
    match name {
        "$top" => Ok(FaceTile::Row(0)),
        "$bottom" => Ok(FaceTile::Row(1)),
        "$side" => Ok(FaceTile::Row(2)),
        _ if name.starts_with('$') => Err(format!(
            "unknown row tile slot '{name}' (expected $top, $bottom or $side)"
        )),
        _ => Tile::from_name(name)
            .map(FaceTile::Tile)
            .ok_or_else(|| format!("unknown tile '{name}'")),
    }
}

fn resolve_part(raw: RawPart) -> Result<ModelPart, String> {
    for a in 0..3 {
        let ok = (-16.0..=32.0).contains(&raw.from[a])
            && (-16.0..=32.0).contains(&raw.to[a])
            && raw.from[a] < raw.to[a];
        if !ok {
            return Err(format!(
                "part extent {:?}..{:?} must be ordered and within -16..=32 texels",
                raw.from, raw.to
            ));
        }
    }
    let all = raw.faces.get("all").map(|t| face_tile(t)).transpose()?;
    let mut faces = [all; 6];
    for (name, tile) in &raw.faces {
        if name != "all" {
            faces[face_index(name)?] = Some(face_tile(tile)?);
        }
    }
    let faces = faces
        .iter()
        .zip(FACE_NAMES)
        .map(|(f, name)| f.ok_or_else(|| format!("part has no tile for its '{name}' face")))
        .collect::<Result<Vec<_>, String>>()?;
    let mut mirror = [false; 6];
    for name in &raw.mirror {
        mirror[face_index(name)?] = true;
    }
    let joint = raw
        .joint
        .map(|j| {
            let axis = match j.axis.as_str() {
                "x" => 0,
                "y" => 1,
                "z" => 2,
                other => return Err(format!("unknown joint axis '{other}' (expected x, y or z)")),
            };
            if !j.open_degrees.is_finite() || j.open_degrees.abs() > 360.0 {
                return Err("joint open_degrees must be within -360..=360".into());
            }
            Ok(Joint {
                axis,
                pivot: j.pivot.map(|v| v / 16.0),
                // Quarter turns stay exact: -90 is exactly -FRAC_PI_2.
                open_radians: j.open_degrees / 90.0 * std::f32::consts::FRAC_PI_2,
            })
        })
        .transpose()?;
    Ok(ModelPart {
        min: raw.from.map(|v| v / 16.0),
        max: raw.to.map(|v| v / 16.0),
        faces: faces.try_into().expect("six faces"),
        mirror,
        thin_edges: raw.thin_edges,
        joint,
    })
}

/// The bounds every pose of `parts` sweeps between closed and fully open,
/// widened to be symmetric about the cell's vertical centre line.
fn swept_bounds(parts: &[ModelPart]) -> Aabb {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for part in parts {
        let samples = if part.joint.is_some() { CULL_SAMPLES } else { 0 };
        for step in 0..=samples {
            let open01 = step as f32 / CULL_SAMPLES as f32;
            for corner in part.corners() {
                let p = match part.joint {
                    Some(j) => j.rotate(corner, j.angle(open01).sin_cos()),
                    None => corner,
                };
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
        }
    }
    let reach = [lo[0], hi[0], lo[2], hi[2]]
        .iter()
        .map(|v| (v - 0.5).abs())
        .fold(0.5f32, f32::max)
        + CULL_MARGIN;
    Aabb {
        min: [0.5 - reach, lo[1] - CULL_MARGIN, 0.5 - reach],
        max: [0.5 + reach, hi[1] + CULL_MARGIN, 0.5 + reach],
    }
}

/// How far the closed variant `0` is lifted to sit centred in a unit cube.
fn item_lift(parts: &[ModelPart]) -> f32 {
    let lo = parts.iter().map(|p| p.min[1]).fold(f32::INFINITY, f32::min);
    let hi = parts.iter().map(|p| p.max[1]).fold(f32::NEG_INFINITY, f32::max);
    (1.0 - (hi - lo)) * 0.5 - lo
}

fn resolve_model(raw: RawModel, key: &'static str) -> Result<AnimatedModelDef, String> {
    let err = |e: String| format!("animated model '{key}': {e}");
    if !raw.open_speed.is_finite() || raw.open_speed <= 0.0 {
        return Err(err("open_speed must be positive".into()));
    }
    if raw.variants.is_empty() || raw.variants.len() > usize::from(u8::MAX) {
        return Err(err("declare 1..=255 variants".into()));
    }
    let variants = raw
        .variants
        .into_iter()
        .map(|v| {
            if v.parts.is_empty() {
                return Err("a variant needs at least one part".to_owned());
            }
            let parts: &'static [ModelPart] = Box::leak(
                v.parts
                    .into_iter()
                    .map(resolve_part)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_boxed_slice(),
            );
            Ok(ModelVariant {
                parts,
                cull: swept_bounds(parts),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(err)?;
    let variants: &'static [ModelVariant] = Box::leak(variants.into_boxed_slice());
    Ok(AnimatedModelDef {
        key,
        open_speed: raw.open_speed,
        item: raw.item,
        item_lift: item_lift(variants[0].parts),
        variants,
    })
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<AnimatedModelDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.animated_models),
        |r| &r.model,
        ENGINE_MODEL_NAMES,
        "animated model",
        |r, id, names| resolve_model(r, names.name(id).expect("id resolved from this table")),
    )
}

#[cfg(test)]
mod tests;
