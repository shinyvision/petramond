use std::collections::HashMap;

use serde::Deserialize;

use crate::block::{Aabb, Block};
use crate::facing::Facing;
use crate::tile::Tile;

const ENGINE_MODEL_NAMES: &[&str] = &["petramond:chest", "petramond:door", "petramond:trapdoor"];

const CULL_MARGIN: f32 = 1.0 / 64.0;
const CULL_SAMPLES: usize = 16;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AnimatedPose {
    pub facing: Facing,
    pub variant: u8,
    pub open: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FaceTile {
    Row(usize),
    Tile(Tile),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Joint {
    pub axis: usize,
    pub pivot: [f32; 3],
    pub open_radians: f32,
}

impl Joint {
    #[inline]
    pub fn angle(&self, open01: f32) -> f32 {
        open01.clamp(0.0, 1.0) * self.open_radians
    }

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

#[derive(Clone, Debug, PartialEq)]
pub struct ModelPart {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub faces: [FaceTile; 6],
    pub mirror: [bool; 6],
    pub thin_edges: bool,
    pub joint: Option<Joint>,
}

impl ModelPart {
    pub fn tiles(&self, block: Block) -> [Tile; 6] {
        let row = block.tiles();
        self.faces.map(|f| match f {
            FaceTile::Row(slot) => row[slot],
            FaceTile::Tile(tile) => tile,
        })
    }

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

#[derive(Clone, Debug, PartialEq)]
pub struct ModelVariant {
    pub parts: &'static [ModelPart],
    pub cull: Aabb,
}

#[derive(Debug, PartialEq)]
pub struct AnimatedModelDef {
    pub key: &'static str,
    pub open_speed: f32,
    pub item: bool,
    pub item_lift: f32,
    variants: &'static [ModelVariant],
}

impl AnimatedModelDef {
    #[inline]
    pub fn variant(&self, index: u8) -> &'static ModelVariant {
        self.variants
            .get(index as usize)
            .unwrap_or(&self.variants[0])
    }

    #[inline]
    pub fn variants(&self) -> &'static [ModelVariant] {
        self.variants
    }
}

pub fn by_key(key: &str) -> Option<&'static AnimatedModelDef> {
    catalog().id(key).map(|id| &catalog().rows()[id as usize])
}

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPart {
    from: [f32; 3],
    to: [f32; 3],
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

fn swept_bounds(parts: &[ModelPart]) -> Aabb {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for part in parts {
        let samples = if part.joint.is_some() {
            CULL_SAMPLES
        } else {
            0
        };
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

fn item_lift(parts: &[ModelPart]) -> f32 {
    let lo = parts.iter().map(|p| p.min[1]).fold(f32::INFINITY, f32::min);
    let hi = parts
        .iter()
        .map(|p| p.max[1])
        .fold(f32::NEG_INFINITY, f32::max);
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
