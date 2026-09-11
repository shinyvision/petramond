use super::effects::{Boundary, Course, Material, RawBoundary, RawCourse, RawMaterial};
use super::effects::{Projection, RawProjection};
use crate::formula::{Expression, Formula};
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};
use serde::{Deserialize, Serialize};

mod bounds;
pub use bounds::{Bounds, RawBounds};

/// Measure a habitat's extent along the six axes before choosing a local shape.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExtentProbe {
    pub step: i32,
    pub vertical_expand: [i32; 2],
    pub horizontal_expand: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawFieldShape {
    #[serde(default)]
    pub admission_offset: [i32; 3],
    #[serde(default)]
    pub surface_offset: i32,
    pub radius: [i32; 2],
    pub height: [i32; 2],
    pub bound_radius: i32,
    pub y: [i32; 2],
    #[serde(default)]
    pub bounds: Option<RawBounds>,
    pub grid_step: i32,
    pub separation: i32,
    #[serde(default)]
    pub extent_probe: Option<ExtentProbe>,
    #[serde(default)]
    pub bindings: Vec<(String, Expression)>,
    /// Positive selects this operation; zero or negative leaves earlier terrain.
    pub cut: Expression,
    /// Index into `palette`, usually an air/fluid/retained-rock selection.
    pub material: Expression,
    pub palette: Vec<RawMaterial>,
    #[serde(default)]
    pub boundaries: Vec<RawBoundary>,
    #[serde(default)]
    pub courses: Vec<RawCourse>,
    #[serde(default)]
    pub projections: Vec<RawProjection>,
    #[serde(default)]
    pub biome_fill: Option<RawBiomeFill>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RawBiomeFill {
    pub y: [i32; 2],
    #[serde(default)]
    pub when: Option<Expression>,
}

pub struct BiomeFill {
    pub y: [i32; 2],
    pub formula: Formula,
    /// The claim's condition as a signed margin, sampled on a lattice.
    pub margin: Formula,
}

pub struct FieldShape {
    pub admission_offset: [i32; 3],
    pub surface_offset: i32,
    pub radius: [i32; 2],
    pub height: [i32; 2],
    pub bound_radius: i32,
    pub y: [i32; 2],
    pub grid_step: i32,
    pub separation: i32,
    pub extent_probe: Option<ExtentProbe>,
    pub formula: Formula,
    /// The cut as a signed margin, positive inside: comparisons become
    /// differences and conjunctions their minimum, so the field can be
    /// sampled on a lattice and interpolated like the cave density.
    pub margin: Formula,
    /// The palette index alone; whether it depends on anything but the site.
    pub material: Formula,
    pub material_varies: bool,
    pub palette: Box<[Material]>,
    pub boundaries: Vec<Boundary>,
    pub courses: Vec<Course>,
    pub projections: Vec<Projection>,
    pub biome_fill: Option<BiomeFill>,
    pub fingerprint: u64,
    pub bounds: Option<Formula>,
}

/// The signed margin of a predicate: how far inside the cut a point is, in
/// the predicate's own units. `lt`/`le` become the difference of their sides
/// (`le` keeps equality inside by half a unit, exact on integer heights),
/// `and` the minimum of its parts, `or` the maximum; any other expression is
/// taken as a margin already (positive selects). A name bound to a predicate
/// is followed so the transformation reaches through bindings.
fn margin_of(cut: &Expression, bindings: &[(String, Expression)], depth: usize) -> Expression {
    let op = |name: &str, parts: Vec<Expression>| {
        let mut all = vec![Expression::Name(name.into())];
        all.extend(parts);
        Expression::Operation(all)
    };
    if depth > 32 {
        return cut.clone();
    }
    match cut {
        Expression::Operation(parts) if parts.len() == 3 => {
            let Expression::Name(name) = &parts[0] else {
                return cut.clone();
            };
            match name.as_str() {
                "lt" => op("sub", vec![parts[2].clone(), parts[1].clone()]),
                "le" => op(
                    "sub",
                    vec![
                        op("add", vec![parts[2].clone(), Expression::Number(0.5)]),
                        parts[1].clone(),
                    ],
                ),
                "gt" => op("sub", vec![parts[1].clone(), parts[2].clone()]),
                "and" => op(
                    "min",
                    vec![
                        margin_of(&parts[1], bindings, depth + 1),
                        margin_of(&parts[2], bindings, depth + 1),
                    ],
                ),
                "or" => op(
                    "max",
                    vec![
                        margin_of(&parts[1], bindings, depth + 1),
                        margin_of(&parts[2], bindings, depth + 1),
                    ],
                ),
                _ => cut.clone(),
            }
        }
        Expression::Name(name) => match bindings.iter().find(|(n, _)| n == name) {
            Some((_, bound)) if matches!(bound, Expression::Operation(parts) if matches!(parts.first(), Some(Expression::Name(op)) if matches!(op.as_str(), "lt" | "le" | "gt" | "and" | "or"))) => {
                margin_of(bound, bindings, depth + 1)
            }
            _ => cut.clone(),
        },
        _ => cut.clone(),
    }
}

impl RawFieldShape {
    pub(super) fn resolve(self, spacing: i32) -> Result<FieldShape, String> {
        super::super::bounds::ascending("radius", (self.radius[0], self.radius[1]), 1..=256)?;
        super::super::bounds::ascending("height", (self.height[0], self.height[1]), 1..=256)?;
        if !(1..=384).contains(&self.bound_radius)
            || self
                .admission_offset
                .iter()
                .any(|v| i64::from(*v).abs() > 32)
            || !(0..=4).contains(&self.surface_offset)
            || self.y[0] < WORLD_MIN_Y
            || self.y[1] >= WORLD_MAX_Y
            || self.y[0] > self.y[1]
            || !(1..=32).contains(&self.grid_step)
            || spacing % self.grid_step != 0
            || self.separation < 0
            || self.separation >= spacing
            || self.separation % self.grid_step != 0
        {
            return Err("invalid field extent or placement grid".into());
        }
        if let Some(p) = &self.extent_probe {
            if !(1..=32).contains(&p.step)
                || p.vertical_expand.iter().any(|v| i64::from(*v).abs() > 64)
                || i64::from(p.horizontal_expand).abs() > 64
            {
                return Err("invalid field extent probe".into());
            }
        }
        if self.palette.is_empty() || self.palette.len() > 16 {
            return Err("field palette needs 1–16 blocks".into());
        }
        let bytes = serde_json::to_vec(&self).map_err(|e| e.to_string())?;
        let bounds = self
            .bounds
            .map(|bounds| bounds.compile(&self.bindings))
            .transpose()?;
        let biome_fill = self
            .biome_fill
            .map(|fill| -> Result<BiomeFill, String> {
                if fill.y[0] < WORLD_MIN_Y || fill.y[1] >= WORLD_MAX_Y || fill.y[0] > fill.y[1] {
                    return Err("'biome_fill.y' must be an ordered world-height band".into());
                }
                let when = fill.when.unwrap_or_else(|| self.cut.clone());
                Ok(BiomeFill {
                    y: fill.y,
                    formula: Formula::compile(&self.bindings, std::slice::from_ref(&when))?,
                    margin: Formula::compile(
                        &self.bindings,
                        &[margin_of(&when, &self.bindings, 0)],
                    )?,
                })
            })
            .transpose()?;
        let palette = self
            .palette
            .into_iter()
            .map(RawMaterial::resolve)
            .collect::<Result<Vec<_>, _>>()?;
        if self.boundaries.len() > 8 || self.courses.len() > 8 || self.projections.len() > 8 {
            return Err("too many field boundary operations".into());
        }
        let boundaries = self
            .boundaries
            .into_iter()
            .map(RawBoundary::resolve)
            .collect::<Result<_, _>>()?;
        let courses = self
            .courses
            .into_iter()
            .map(|course| course.resolve(&self.bindings))
            .collect::<Result<_, _>>()?;
        let projections = self
            .projections
            .into_iter()
            .map(|p| p.resolve(&self.bindings))
            .collect::<Result<_, _>>()?;
        let material = Formula::compile(&self.bindings, std::slice::from_ref(&self.material))?;
        Ok(FieldShape {
            admission_offset: self.admission_offset,
            surface_offset: self.surface_offset,
            radius: self.radius,
            height: self.height,
            bound_radius: self.bound_radius,
            y: self.y,
            grid_step: self.grid_step,
            separation: self.separation,
            formula: Formula::compile(&self.bindings, &[self.cut.clone(), self.material.clone()])?,
            margin: Formula::compile(&self.bindings, &[margin_of(&self.cut, &self.bindings, 0)])?,
            material_varies: material.uses_any(&[0, 1, 2, 8]),
            material,
            palette: palette.into(),
            boundaries,
            courses,
            projections,
            extent_probe: self.extent_probe,
            biome_fill,
            bounds,
            fingerprint: super::load::hash(&bytes),
        })
    }
}
