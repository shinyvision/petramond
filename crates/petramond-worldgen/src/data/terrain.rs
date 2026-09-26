//! The terrain density recipe — a layered catalog
//! (`assets/density/terrain.json`): the scalar graph of named channels the
//! surface fill, the biome classifier and the cave field sample (see
//! [`crate::density::terrain`]), written as data and interpreted here.
//!
//! ```json
//! "fields":   {"crag": {"salt": [1271071581156483110, 506583420214636557],
//!                       "first_octave": -5, "amplitudes": [1.0, 1.0]}},
//! "splines":  {"offset": {"axis": "continentality", "points": [
//!                [-1.1, 0.044, 0.0], [-0.16, "offset_coast", 0.0]]}},
//! "nodes":    {"crest": {"clamp": {"input": {"add": [
//!                {"multiply": [{"abs": {"field": "crag"}}, -2.0]}, 1.0]},
//!                "min": 0.0, "max": 1.0}}},
//! "channels": {"base_height": "base_height", "surface_detection": 0.0}
//! ```
//!
//! A `field` is a seeded double-Perlin climate noise (forked from the world
//! seed by its `salt`, sampled with the shared climate domain warp). A spline
//! is an `axis` plus `[location, value, slope]` knots (slope optional; a
//! value is a number or a nested spline, by name or inline). Every operand
//! of a node — and every channel — is a node NAME, a number (a constant) or
//! an inline node. Node vocabulary: `constant`, `field`, `axis` (`x`/`y`/`z`),
//! `add` / `multiply` / `min` / `max` (`[a, b]`), `abs`, `ridge_fold`,
//! `vertical_bias` (one operand), `terrace` `{input, step}`, `clamp`
//! `{input, min, max}`, `lerp` `{a, b, t}`, `vertical_ramp` `{y_min, y_max}`,
//! `floor_clamp` `{input, floor_y, fade_height, solid_density}`,
//! `range_select` `{selector, min, max, inside, outside}` and `spline`
//! `{spline, inputs: {axis: operand}}`. Nodes resolve from the channels
//! down, so authoring order is free; a cycle is an error.
//!
//! Layering: every section merges by name — a later layer replaces a field,
//! spline, node or channel it restates, so a pack retunes one constant or
//! swaps a whole recipe. The loaded recipe must expose the climate channels,
//! `base_height` (all horizontal) and `master_density`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::Fingerprint;
use crate::density::noise::{ClimateFieldParams, ShiftedClimateField};
use crate::density::terrain::channels;
use crate::graph::spline::{CubicSpline, SplineAxis, SplinePoint, SplineValue};
use crate::graph::{Axis, Channel, NodeId, ScalarGraph};

/// Channels the rest of worldgen samples. All but `master_density` must be
/// horizontal (Y-invariant): they are sampled once per column.
const REQUIRED_CHANNELS: [(&str, bool); 7] = [
    (channels::TEMPERATURE, true),
    (channels::HUMIDITY, true),
    (channels::CONTINENTALITY, true),
    (channels::EROSION, true),
    (channels::VARIANCE, true),
    (channels::BASE_HEIGHT, true),
    (channels::MASTER_DENSITY, false),
];

/// Octave-table limits of the reference double-Perlin a field builds.
const MAX_FIELD_OCTAVES: usize = 9;
const MIN_FIRST_OCTAVE: i32 = -12;

/// One layer of the recipe (and the merged recipe).
#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawRecipe {
    #[serde(default)]
    fields: BTreeMap<String, RawField>,
    #[serde(default)]
    splines: BTreeMap<String, RawSpline>,
    #[serde(default)]
    nodes: BTreeMap<String, RawOp>,
    #[serde(default)]
    channels: BTreeMap<String, RawOperand>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawField {
    salt: (u64, u64),
    first_octave: i32,
    amplitudes: Vec<f64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RawSpline {
    axis: String,
    points: Vec<RawKnot>,
}

/// `[location, value]` or `[location, value, slope]`.
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum RawKnot {
    Sloped(f64, RawKnotValue, f64),
    Plain(f64, RawKnotValue),
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum RawKnotValue {
    Constant(f64),
    Spline(RawSplineRef),
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum RawSplineRef {
    Named(String),
    Inline(Box<RawSpline>),
}

/// A node input: a node name, a constant, or an inline node.
#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum RawOperand {
    Node(String),
    Constant(f64),
    Inline(Box<RawOp>),
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RawAxis {
    X,
    Y,
    Z,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RawOp {
    Constant(f64),
    Field(String),
    Axis(RawAxis),
    Add([RawOperand; 2]),
    Multiply([RawOperand; 2]),
    Min([RawOperand; 2]),
    Max([RawOperand; 2]),
    Abs(RawOperand),
    RidgeFold(RawOperand),
    VerticalBias(RawOperand),
    Terrace {
        input: RawOperand,
        step: f64,
    },
    Clamp {
        input: RawOperand,
        min: f64,
        max: f64,
    },
    Lerp {
        a: RawOperand,
        b: RawOperand,
        t: RawOperand,
    },
    VerticalRamp {
        y_min: f64,
        y_max: f64,
    },
    FloorClamp {
        input: RawOperand,
        floor_y: f64,
        fade_height: f64,
        solid_density: f64,
    },
    RangeSelect {
        selector: RawOperand,
        min: f64,
        max: f64,
        inside: RawOperand,
        outside: RawOperand,
    },
    Spline {
        spline: RawSplineRef,
        inputs: BTreeMap<String, RawOperand>,
    },
}

/// A loaded, validated terrain recipe: builds the density graph for a seed.
pub struct TerrainRecipe {
    recipe: RawRecipe,
    /// Octave amplitudes per field, stored for the process (the noise
    /// parameters borrow them for `'static`).
    amplitudes: BTreeMap<String, &'static [f64]>,
    /// Hash of the merged recipe, stamped into the column-gen cache: a pack
    /// that reshapes terrain must not be served stale cached columns.
    pub fingerprint: u64,
}

fn finite(what: &str, value: f64) -> Result<f64, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("{what} must be finite"))
    }
}

fn validate_field(name: &str, field: &RawField) -> Result<(), String> {
    let n = field.amplitudes.len();
    let last_octave = field.first_octave + n as i32 - 1;
    if n == 0
        || n > MAX_FIELD_OCTAVES
        || field.first_octave < MIN_FIRST_OCTAVE
        || last_octave > 0
        || field.amplitudes.iter().any(|a| !a.is_finite())
        || field.amplitudes.iter().all(|&a| a == 0.0)
    {
        return Err(format!(
            "field '{name}': needs 1..={MAX_FIELD_OCTAVES} finite amplitudes (not all zero) \
             over octaves {MIN_FIRST_OCTAVE}..=0"
        ));
    }
    Ok(())
}

/// Resolves one recipe into one graph: nodes and splines by name, each node
/// built once and shared by every reference.
struct GraphBuilder<'a> {
    recipe: &'a TerrainRecipe,
    seed: u64,
    graph: ScalarGraph,
    built: BTreeMap<&'a str, NodeId>,
    resolving: BTreeSet<&'a str>,
    splines_resolving: BTreeSet<&'a str>,
}

impl<'a> GraphBuilder<'a> {
    fn operand(&mut self, operand: &'a RawOperand) -> Result<NodeId, String> {
        match operand {
            RawOperand::Node(name) => self.node(name),
            RawOperand::Constant(value) => Ok(self.graph.constant(finite("a constant", *value)?)),
            RawOperand::Inline(op) => self.op(op),
        }
    }

    fn node(&mut self, name: &'a str) -> Result<NodeId, String> {
        if let Some(&id) = self.built.get(name) {
            return Ok(id);
        }
        let recipe = self.recipe;
        let op = recipe
            .recipe
            .nodes
            .get(name)
            .ok_or_else(|| format!("unknown node '{name}'"))?;
        if !self.resolving.insert(name) {
            return Err(format!("node '{name}' depends on itself"));
        }
        let id = self.op(op).map_err(|e| format!("node '{name}': {e}"))?;
        self.resolving.remove(name);
        self.built.insert(name, id);
        Ok(id)
    }

    fn pair(&mut self, [a, b]: &'a [RawOperand; 2]) -> Result<(NodeId, NodeId), String> {
        Ok((self.operand(a)?, self.operand(b)?))
    }

    fn op(&mut self, op: &'a RawOp) -> Result<NodeId, String> {
        Ok(match op {
            RawOp::Constant(value) => self.graph.constant(finite("a constant", *value)?),
            RawOp::Field(name) => {
                let recipe = self.recipe;
                let field = recipe
                    .recipe
                    .fields
                    .get(name)
                    .ok_or_else(|| format!("unknown field '{name}'"))?;
                let params = ClimateFieldParams {
                    salt: field.salt,
                    omin: field.first_octave,
                    amplitudes: recipe.amplitudes[name.as_str()],
                };
                self.graph.sampled_field(ShiftedClimateField::new(self.seed, &params))
            }
            RawOp::Axis(axis) => self.graph.axis(match axis {
                RawAxis::X => Axis::X,
                RawAxis::Y => Axis::Y,
                RawAxis::Z => Axis::Z,
            }),
            RawOp::Add(pair) => {
                let (a, b) = self.pair(pair)?;
                self.graph.add(a, b)
            }
            RawOp::Multiply(pair) => {
                let (a, b) = self.pair(pair)?;
                self.graph.multiply(a, b)
            }
            RawOp::Min(pair) => {
                let (a, b) = self.pair(pair)?;
                self.graph.min(a, b)
            }
            RawOp::Max(pair) => {
                let (a, b) = self.pair(pair)?;
                self.graph.max(a, b)
            }
            RawOp::Abs(input) => {
                let input = self.operand(input)?;
                self.graph.abs(input)
            }
            RawOp::RidgeFold(input) => {
                let input = self.operand(input)?;
                self.graph.ridge_fold(input)
            }
            RawOp::VerticalBias(base_height) => {
                let base_height = self.operand(base_height)?;
                self.graph.vertical_bias(base_height)
            }
            RawOp::Terrace { input, step } => {
                let step = finite("terrace step", *step)?;
                if step <= 0.0 {
                    return Err("terrace step must be positive".into());
                }
                let input = self.operand(input)?;
                self.graph.terrace(input, step)
            }
            RawOp::Clamp { input, min, max } => {
                let (min, max) = (finite("clamp min", *min)?, finite("clamp max", *max)?);
                let input = self.operand(input)?;
                self.graph.clamp(input, min, max)
            }
            RawOp::Lerp { a, b, t } => {
                let (a, b) = (self.operand(a)?, self.operand(b)?);
                let t = self.operand(t)?;
                self.graph.lerp(a, b, t)
            }
            RawOp::VerticalRamp { y_min, y_max } => self.graph.vertical_ramp(
                finite("vertical_ramp y_min", *y_min)?,
                finite("vertical_ramp y_max", *y_max)?,
            ),
            RawOp::FloorClamp {
                input,
                floor_y,
                fade_height,
                solid_density,
            } => {
                let floor_y = finite("floor_clamp floor_y", *floor_y)?;
                let fade_height = finite("floor_clamp fade_height", *fade_height)?;
                let solid_density = finite("floor_clamp solid_density", *solid_density)?;
                let input = self.operand(input)?;
                self.graph.floor_clamp(input, floor_y, fade_height, solid_density)
            }
            RawOp::RangeSelect {
                selector,
                min,
                max,
                inside,
                outside,
            } => {
                let (min, max) = (finite("range min", *min)?, finite("range max", *max)?);
                let selector = self.operand(selector)?;
                let (inside, outside) = (self.operand(inside)?, self.operand(outside)?);
                self.graph.range_select(selector, min, max, inside, outside)
            }
            RawOp::Spline { spline, inputs } => {
                let spline = self.spline(spline)?;
                let bound: BTreeSet<SplineAxis> =
                    inputs.keys().map(|axis| SplineAxis::new(axis.as_str())).collect();
                let required = spline.required_axes();
                if let Some(axis) = required.difference(&bound).next() {
                    return Err(format!("spline input axis '{}' is unbound", axis.as_str()));
                }
                if let Some(axis) = bound.difference(&required).next() {
                    return Err(format!("spline input axis '{}' is not used", axis.as_str()));
                }
                let mut bound_inputs = Vec::with_capacity(inputs.len());
                for (axis, operand) in inputs {
                    bound_inputs.push((SplineAxis::new(axis.as_str()), self.operand(operand)?));
                }
                self.graph.spline(spline, bound_inputs)
            }
        })
    }

    fn spline(&mut self, spline: &'a RawSplineRef) -> Result<CubicSpline, String> {
        match spline {
            RawSplineRef::Inline(spline) => self.spline_body(spline),
            RawSplineRef::Named(name) => {
                let recipe = self.recipe;
                let body = recipe
                    .recipe
                    .splines
                    .get(name)
                    .ok_or_else(|| format!("unknown spline '{name}'"))?;
                if !self.splines_resolving.insert(name.as_str()) {
                    return Err(format!("spline '{name}' nests itself"));
                }
                let spline = self
                    .spline_body(body)
                    .map_err(|e| format!("spline '{name}': {e}"))?;
                self.splines_resolving.remove(name.as_str());
                Ok(spline)
            }
        }
    }

    fn spline_body(&mut self, spline: &'a RawSpline) -> Result<CubicSpline, String> {
        if spline.axis.is_empty() || spline.points.is_empty() {
            return Err("a spline needs an axis and at least one knot".into());
        }
        let mut points = Vec::with_capacity(spline.points.len());
        let mut previous = f64::NEG_INFINITY;
        for knot in &spline.points {
            let (location, value, slope) = match knot {
                RawKnot::Sloped(location, value, slope) => (location, value, Some(*slope)),
                RawKnot::Plain(location, value) => (location, value, None),
            };
            let location = finite("a knot location", *location)?;
            if location <= previous {
                return Err("knot locations must strictly increase".into());
            }
            previous = location;
            if let Some(slope) = slope {
                finite("a knot slope", slope)?;
            }
            let value = match value {
                RawKnotValue::Constant(value) => {
                    SplineValue::Constant(finite("a knot value", *value)?)
                }
                RawKnotValue::Spline(nested) => SplineValue::Spline(Box::new(self.spline(nested)?)),
            };
            points.push(SplinePoint::with_optional_derivative(location, value, slope));
        }
        Ok(CubicSpline::new(spline.axis.as_str(), points))
    }
}

impl TerrainRecipe {
    fn new(recipe: RawRecipe) -> Result<Self, String> {
        for (name, field) in &recipe.fields {
            validate_field(name, field)?;
        }
        let fingerprint = {
            let mut hash = Fingerprint::new();
            hash.eat(&serde_json::to_vec(&recipe).map_err(|e| e.to_string())?);
            hash.finish()
        };
        let amplitudes = recipe
            .fields
            .iter()
            .map(|(name, field)| (name.clone(), &*field.amplitudes.clone().leak()))
            .collect();
        let loaded = Self {
            recipe,
            amplitudes,
            fingerprint,
        };
        // Structure does not depend on the seed: one build proves them all.
        loaded.try_build(0)?;
        Ok(loaded)
    }

    fn try_build(&self, seed: u32) -> Result<ScalarGraph, String> {
        let mut builder = GraphBuilder {
            recipe: self,
            seed: u64::from(seed),
            graph: ScalarGraph::new(),
            built: BTreeMap::new(),
            resolving: BTreeSet::new(),
            splines_resolving: BTreeSet::new(),
        };
        for (channel, operand) in &self.recipe.channels {
            if channel.is_empty() {
                return Err("a channel needs a name".into());
            }
            let node = builder
                .operand(operand)
                .map_err(|e| format!("channel '{channel}': {e}"))?;
            builder.graph.set_channel(Channel::new(channel.as_str()), node);
        }
        let graph = builder.graph;
        for (channel, horizontal) in REQUIRED_CHANNELS {
            let node = graph
                .channel_node(channel)
                .ok_or_else(|| format!("the recipe has no '{channel}' channel"))?;
            if horizontal && graph.node_depends_on_y(node) {
                return Err(format!("channel '{channel}' must not depend on Y"));
            }
        }
        Ok(graph)
    }

    /// The density graph for `seed`.
    pub fn build(&self, seed: u32) -> ScalarGraph {
        self.try_build(seed).expect("the terrain recipe was validated at load")
    }
}

/// Merge the layers (base first) by name and validate the result.
fn parse_layers(texts: &[&str]) -> Result<TerrainRecipe, String> {
    let mut merged = RawRecipe::default();
    for (i, text) in texts.iter().enumerate() {
        let layer: RawRecipe =
            serde_json::from_str(text).map_err(|e| format!("layer #{i}: {e}"))?;
        merged.fields.extend(layer.fields);
        merged.splines.extend(layer.splines);
        merged.nodes.extend(layer.nodes);
        merged.channels.extend(layer.channels);
    }
    TerrainRecipe::new(merged)
}

/// The terrain recipe stage (see [`super::content_stages`]); a missing or
/// malformed layer fails the registry build.
pub(crate) static RECIPE: petramond_world::content::Slot<TerrainRecipe> =
    petramond_world::content::Slot::new("density/terrain.json", &[], load_recipe);

fn load_recipe(reg: &petramond_world::content::ContentRegistry) -> Result<TerrainRecipe, String> {
    petramond_world::registry::read_catalog(
        reg.packs(),
        "density/terrain.json",
        "terrain density recipe",
        parse_layers,
    )
}

/// The current registry's terrain recipe.
pub fn recipe() -> &'static TerrainRecipe {
    RECIPE.current()
}

#[cfg(test)]
mod tests;
