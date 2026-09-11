//! Material operations around a field's exposed boundary.

use crate::formula::{Expression, Formula};
use petramond_world::block::Block;
use serde::{Deserialize, Serialize};

mod projection;
pub use projection::{Projection, RawProjection};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialFilter {
    #[default]
    Any,
    Solid,
    Fluid,
}

impl MaterialFilter {
    pub fn accepts(self, block: u16) -> bool {
        let block = Block::from_id(block);
        match self {
            Self::Any => true,
            Self::Solid => block.is_solid(),
            Self::Fluid => block.is_fluid(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RawMaterial {
    Block(String),
    Filtered {
        block: String,
        replace: MaterialFilter,
    },
}

#[derive(Clone, Copy)]
pub struct Material {
    pub block: u16,
    pub replace: MaterialFilter,
}

fn block(name: &str) -> Result<u16, String> {
    serde_json::from_value::<Block>(serde_json::Value::String(name.into()))
        .map(|b| b.id())
        .map_err(|_| format!("unknown field block '{name}'"))
}

impl RawMaterial {
    pub(super) fn resolve(self) -> Result<Material, String> {
        let (name, replace) = match self {
            Self::Block(name) => (name, MaterialFilter::Any),
            Self::Filtered { block, replace } => (block, replace),
        };
        let id = block(&name)?;
        let b = Block::from_id(id);
        if b != Block::Air && !b.is_solid() && !b.is_fluid() {
            return Err(format!("field block '{name}' must be air, solid, or fluid"));
        }
        Ok(Material { block: id, replace })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawBoundary {
    pub from: Vec<String>,
    pub offsets: Vec<[i32; 3]>,
    pub block: String,
    #[serde(default)]
    pub replace: MaterialFilter,
    #[serde(default)]
    pub avoid: Vec<String>,
}

pub struct Boundary {
    pub from: Vec<u16>,
    pub offsets: Vec<[i32; 3]>,
    pub block: u16,
    pub replace: MaterialFilter,
    pub avoid: Vec<u16>,
}

impl RawBoundary {
    pub(super) fn resolve(self) -> Result<Boundary, String> {
        if self.from.is_empty()
            || self.from.len() > 16
            || self.offsets.is_empty()
            || self.offsets.len() > 6
            || self
                .offsets
                .iter()
                .any(|p| p.iter().map(|v| i64::from(*v).abs()).sum::<i64>() != 1)
        {
            return Err("a boundary needs source materials and unit cardinal offsets".into());
        }
        Ok(Boundary {
            from: self
                .from
                .iter()
                .map(|s| block(s))
                .collect::<Result<_, _>>()?,
            offsets: self.offsets,
            block: block(&self.block)?,
            replace: self.replace,
            avoid: self
                .avoid
                .iter()
                .map(|s| block(s))
                .collect::<Result<_, _>>()?,
        })
    }
}

/// An exposed face anchors a short axial course. The palette's last material repeats.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawCourse {
    pub from: Vec<String>,
    pub offset: [i32; 3],
    pub step: [i32; 3],
    pub length: [u8; 2],
    pub palette: Vec<String>,
    #[serde(default)]
    pub avoid: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<(String, Expression)>,
    #[serde(default)]
    pub extent: Option<Expression>,
    #[serde(default)]
    pub when: Option<Expression>,
    #[serde(default)]
    pub remap: Vec<[String; 2]>,
    #[serde(default)]
    pub patches: Vec<RawPatch>,
}

/// A variant a course block turns into where a noise field is high enough:
/// moss on grass, packed mud in dirt, in organic patches.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawPatch {
    pub from: String,
    pub to: String,
    pub scale: f64,
    pub above: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub from: u16,
    pub to: u16,
    pub scale: f64,
    pub above: f64,
}

pub struct Course {
    pub from: Vec<u16>,
    pub offset: [i32; 3],
    pub step: [i32; 3],
    pub length: [u8; 2],
    pub palette: Vec<u16>,
    pub avoid: Vec<u16>,
    pub formula: Formula,
    pub has_extent: bool,
    pub remap: Vec<[u16; 2]>,
    pub patches: Vec<Patch>,
}

impl Course {
    /// The block a course lays at `pos` once its patches have had their say.
    pub fn patched(&self, block: u16, seed: u32, pos: [i32; 3]) -> u16 {
        for patch in &self.patches {
            if patch.from != block {
                continue;
            }
            let shift = f64::from(seed % 4096) * 17.0;
            let p = pos.map(|v| f64::from(v) / patch.scale);
            if petramond_math::noise::simplex3([p[0] + shift, p[1], p[2] - shift]) > patch.above {
                return patch.to;
            }
        }
        block
    }
}

impl RawCourse {
    pub(super) fn resolve(self, parent: &[(String, Expression)]) -> Result<Course, String> {
        if self.from.is_empty()
            || self.from.len() > 16
            || self.palette.is_empty()
            || self.palette.len() > 8
            || self.remap.len() > 16
            || self.patches.len() > 8
            || self
                .patches
                .iter()
                .any(|p| !(1.0..=64.0).contains(&p.scale) || !p.above.is_finite())
            || self.length[0] == 0
            || self.length[0] > self.length[1]
            || self.length[1] > 4
            || [self.offset, self.step]
                .iter()
                .any(|p| p.iter().map(|v| i64::from(*v).abs()).sum::<i64>() != 1)
        {
            return Err("invalid exposed course: cardinal steps and 1–4 cells required".into());
        }
        let resolve = |names: Vec<String>| {
            names
                .iter()
                .map(|s| block(s))
                .collect::<Result<Vec<_>, _>>()
        };
        let mut bindings = parent.to_vec();
        bindings.extend(self.bindings);
        Ok(Course {
            from: resolve(self.from)?,
            offset: self.offset,
            step: self.step,
            length: self.length,
            palette: resolve(self.palette)?,
            avoid: resolve(self.avoid)?,
            has_extent: self.extent.is_some(),
            remap: self
                .remap
                .into_iter()
                .map(|[a, b]| Ok([block(&a)?, block(&b)?]))
                .collect::<Result<_, String>>()?,
            patches: self
                .patches
                .into_iter()
                .map(|p| {
                    Ok(Patch {
                        from: block(&p.from)?,
                        to: block(&p.to)?,
                        scale: p.scale,
                        above: p.above,
                    })
                })
                .collect::<Result<_, String>>()?,
            formula: Formula::compile(
                &bindings,
                &[
                    self.extent.unwrap_or(Expression::Number(1.0)),
                    self.when.unwrap_or(Expression::Number(1.0)),
                ],
            )?,
        })
    }
}
