//! Pure scalar graph primitives for staged worldgen fields.

pub(crate) mod spline;

use self::spline::{CubicSpline, SplineAxis};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_GRAPH_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct SamplePoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl SamplePoint {
    pub(crate) const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Channel(String);

impl Channel {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        assert!(
            !name.is_empty(),
            "worldgen graph channel names cannot be empty"
        );
        Self(name)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Channel {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for Channel {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
struct GraphId(u64);

impl GraphId {
    fn next() -> Self {
        Self(NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct NodeId {
    graph_id: GraphId,
    index: usize,
}

impl NodeId {
    fn new(graph_id: GraphId, index: usize) -> Self {
        Self { graph_id, index }
    }

    fn with_graph(self, graph_id: GraphId) -> Self {
        Self {
            graph_id,
            index: self.index,
        }
    }
}

#[cfg(test)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum Axis {
    X,
    Y,
    Z,
}

pub(crate) trait SampledScalarField: Debug + Send + Sync {
    fn sample(&self, point: SamplePoint) -> f64;

    fn depends_on_y(&self) -> bool {
        true
    }
}

#[derive(Clone, Debug)]
enum Node {
    Constant(f64),
    #[cfg(test)]
    Axis(Axis),
    SampledField(Arc<dyn SampledScalarField>),
    Add(NodeId, NodeId),
    Multiply(NodeId, NodeId),
    #[cfg(test)]
    Min(NodeId, NodeId),
    #[cfg(test)]
    Max(NodeId, NodeId),
    #[cfg(test)]
    Abs(NodeId),
    RidgeFold(NodeId),
    #[cfg(test)]
    Clamp {
        input: NodeId,
        min: f64,
        max: f64,
    },
    #[cfg(test)]
    Lerp {
        a: NodeId,
        b: NodeId,
        t: NodeId,
    },
    #[cfg(test)]
    VerticalRamp {
        y_min: f64,
        y_max: f64,
    },
    VerticalBias {
        base_height: NodeId,
    },
    FloorClamp {
        input: NodeId,
        floor_y: f64,
        fade_height: f64,
        solid_density: f64,
    },
    #[cfg(test)]
    RangeSelect {
        selector: NodeId,
        min: f64,
        max: f64,
        inside: NodeId,
        outside: NodeId,
    },
    Spline {
        spline: Box<CubicSpline>,
        inputs: Vec<(SplineAxis, NodeId)>,
    },
}

impl Node {
    fn with_graph(self, graph_id: GraphId) -> Self {
        match self {
            Self::Constant(value) => Self::Constant(value),
            #[cfg(test)]
            Self::Axis(axis) => Self::Axis(axis),
            Self::SampledField(field) => Self::SampledField(field),
            Self::Add(a, b) => Self::Add(a.with_graph(graph_id), b.with_graph(graph_id)),
            Self::Multiply(a, b) => Self::Multiply(a.with_graph(graph_id), b.with_graph(graph_id)),
            #[cfg(test)]
            Self::Min(a, b) => Self::Min(a.with_graph(graph_id), b.with_graph(graph_id)),
            #[cfg(test)]
            Self::Max(a, b) => Self::Max(a.with_graph(graph_id), b.with_graph(graph_id)),
            #[cfg(test)]
            Self::Abs(input) => Self::Abs(input.with_graph(graph_id)),
            Self::RidgeFold(input) => Self::RidgeFold(input.with_graph(graph_id)),
            #[cfg(test)]
            Self::Clamp { input, min, max } => Self::Clamp {
                input: input.with_graph(graph_id),
                min,
                max,
            },
            #[cfg(test)]
            Self::Lerp { a, b, t } => Self::Lerp {
                a: a.with_graph(graph_id),
                b: b.with_graph(graph_id),
                t: t.with_graph(graph_id),
            },
            #[cfg(test)]
            Self::VerticalRamp { y_min, y_max } => Self::VerticalRamp { y_min, y_max },
            Self::VerticalBias { base_height } => Self::VerticalBias {
                base_height: base_height.with_graph(graph_id),
            },
            Self::FloorClamp {
                input,
                floor_y,
                fade_height,
                solid_density,
            } => Self::FloorClamp {
                input: input.with_graph(graph_id),
                floor_y,
                fade_height,
                solid_density,
            },
            #[cfg(test)]
            Self::RangeSelect {
                selector,
                min,
                max,
                inside,
                outside,
            } => Self::RangeSelect {
                selector: selector.with_graph(graph_id),
                min,
                max,
                inside: inside.with_graph(graph_id),
                outside: outside.with_graph(graph_id),
            },
            Self::Spline { spline, inputs } => Self::Spline {
                spline,
                inputs: inputs
                    .into_iter()
                    .map(|(axis, node)| (axis, node.with_graph(graph_id)))
                    .collect(),
            },
        }
    }
}

#[derive(Debug)]
pub(crate) struct ScalarGraph {
    graph_id: GraphId,
    nodes: Vec<Node>,
    y_dependencies: Vec<bool>,
    outputs: BTreeMap<String, NodeId>,
}

#[derive(Debug)]
pub(crate) struct GraphEvaluationCache {
    graph_id: GraphId,
    values: Vec<f64>,
    stamps: Vec<u32>,
    generation: u32,
    y_invariant_values: Vec<f64>,
    y_invariant_stamps: Vec<u32>,
    y_invariant_generation: u32,
    y_invariant_scope_active: bool,
}

impl GraphEvaluationCache {
    fn new(graph: &ScalarGraph) -> Self {
        Self {
            graph_id: graph.graph_id,
            values: vec![0.0; graph.nodes.len()],
            stamps: vec![0; graph.nodes.len()],
            generation: 0,
            y_invariant_values: vec![0.0; graph.nodes.len()],
            y_invariant_stamps: vec![0; graph.nodes.len()],
            y_invariant_generation: 0,
            y_invariant_scope_active: false,
        }
    }

    fn begin_sample(&mut self, graph: &ScalarGraph) {
        self.ensure_graph_capacity(graph);
        advance_generation(&mut self.generation, &mut self.stamps);
    }

    pub(crate) fn begin_y_invariant_column(&mut self, graph: &ScalarGraph) {
        self.ensure_graph_capacity(graph);
        self.y_invariant_scope_active = true;
        advance_generation(
            &mut self.y_invariant_generation,
            &mut self.y_invariant_stamps,
        );
    }

    fn ensure_graph_capacity(&mut self, graph: &ScalarGraph) {
        assert_eq!(
            self.graph_id, graph.graph_id,
            "graph evaluation cache belongs to a different scalar graph"
        );
        if self.values.len() < graph.nodes.len() {
            self.values.resize(graph.nodes.len(), 0.0);
            self.stamps.resize(graph.nodes.len(), 0);
            self.y_invariant_values.resize(graph.nodes.len(), 0.0);
            self.y_invariant_stamps.resize(graph.nodes.len(), 0);
        }
    }

    fn get(&self, node: NodeId, y_invariant: bool) -> Option<f64> {
        if self.use_y_invariant_scope(y_invariant) {
            (self.y_invariant_stamps[node.index] == self.y_invariant_generation)
                .then_some(self.y_invariant_values[node.index])
        } else {
            (self.stamps[node.index] == self.generation).then_some(self.values[node.index])
        }
    }

    fn store(&mut self, node: NodeId, value: f64, y_invariant: bool) -> f64 {
        if self.use_y_invariant_scope(y_invariant) {
            self.y_invariant_values[node.index] = value;
            self.y_invariant_stamps[node.index] = self.y_invariant_generation;
        } else {
            self.values[node.index] = value;
            self.stamps[node.index] = self.generation;
        }
        value
    }

    fn use_y_invariant_scope(&self, y_invariant: bool) -> bool {
        y_invariant && self.y_invariant_scope_active
    }
}

impl Default for ScalarGraph {
    fn default() -> Self {
        Self {
            graph_id: GraphId::next(),
            nodes: Vec::new(),
            y_dependencies: Vec::new(),
            outputs: BTreeMap::new(),
        }
    }
}

impl Clone for ScalarGraph {
    fn clone(&self) -> Self {
        let graph_id = GraphId::next();
        Self {
            graph_id,
            nodes: self
                .nodes
                .iter()
                .map(|node| node.clone().with_graph(graph_id))
                .collect(),
            y_dependencies: self.y_dependencies.clone(),
            outputs: self
                .outputs
                .iter()
                .map(|(channel, node)| (channel.clone(), node.with_graph(graph_id)))
                .collect(),
        }
    }
}

impl ScalarGraph {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn constant(&mut self, value: f64) -> NodeId {
        self.push(Node::Constant(value))
    }

    #[cfg(test)]
    pub(crate) fn axis(&mut self, axis: Axis) -> NodeId {
        self.push(Node::Axis(axis))
    }

    pub(crate) fn sampled_field(&mut self, field: impl SampledScalarField + 'static) -> NodeId {
        self.push(Node::SampledField(Arc::new(field)))
    }

    pub(crate) fn add(&mut self, a: NodeId, b: NodeId) -> NodeId {
        self.assert_existing_node(a, "add left input");
        self.assert_existing_node(b, "add right input");
        self.push(Node::Add(a, b))
    }

    pub(crate) fn multiply(&mut self, a: NodeId, b: NodeId) -> NodeId {
        self.assert_existing_node(a, "multiply left input");
        self.assert_existing_node(b, "multiply right input");
        self.push(Node::Multiply(a, b))
    }

    #[cfg(test)]
    pub(crate) fn min(&mut self, a: NodeId, b: NodeId) -> NodeId {
        self.assert_existing_node(a, "min left input");
        self.assert_existing_node(b, "min right input");
        self.push(Node::Min(a, b))
    }

    #[cfg(test)]
    pub(crate) fn max(&mut self, a: NodeId, b: NodeId) -> NodeId {
        self.assert_existing_node(a, "max left input");
        self.assert_existing_node(b, "max right input");
        self.push(Node::Max(a, b))
    }

    #[cfg(test)]
    pub(crate) fn abs(&mut self, input: NodeId) -> NodeId {
        self.assert_existing_node(input, "abs input");
        self.push(Node::Abs(input))
    }

    pub(crate) fn ridge_fold(&mut self, input: NodeId) -> NodeId {
        self.assert_existing_node(input, "ridge fold input");
        self.push(Node::RidgeFold(input))
    }

    #[cfg(test)]
    pub(crate) fn clamp(&mut self, input: NodeId, min: f64, max: f64) -> NodeId {
        self.assert_existing_node(input, "clamp input");
        self.push(Node::Clamp { input, min, max })
    }

    #[cfg(test)]
    pub(crate) fn lerp(&mut self, a: NodeId, b: NodeId, t: NodeId) -> NodeId {
        self.assert_existing_node(a, "lerp first input");
        self.assert_existing_node(b, "lerp second input");
        self.assert_existing_node(t, "lerp selector input");
        self.push(Node::Lerp { a, b, t })
    }

    #[cfg(test)]
    pub(crate) fn vertical_ramp(&mut self, y_min: f64, y_max: f64) -> NodeId {
        self.push(Node::VerticalRamp { y_min, y_max })
    }

    pub(crate) fn vertical_bias(&mut self, base_height: NodeId) -> NodeId {
        self.assert_existing_node(base_height, "vertical bias base height input");
        self.push(Node::VerticalBias { base_height })
    }

    pub(crate) fn floor_clamp(
        &mut self,
        input: NodeId,
        floor_y: f64,
        fade_height: f64,
        solid_density: f64,
    ) -> NodeId {
        self.assert_existing_node(input, "floor clamp input");
        self.push(Node::FloorClamp {
            input,
            floor_y,
            fade_height,
            solid_density,
        })
    }

    #[cfg(test)]
    pub(crate) fn range_select(
        &mut self,
        selector: NodeId,
        min: f64,
        max: f64,
        inside: NodeId,
        outside: NodeId,
    ) -> NodeId {
        self.assert_existing_node(selector, "range selector input");
        self.assert_existing_node(inside, "range inside input");
        self.assert_existing_node(outside, "range outside input");
        self.push(Node::RangeSelect {
            selector,
            min,
            max,
            inside,
            outside,
        })
    }

    pub(crate) fn spline(
        &mut self,
        spline: CubicSpline,
        inputs: impl Into<Vec<(SplineAxis, NodeId)>>,
    ) -> NodeId {
        let inputs = inputs.into();
        for required_axis in spline.required_axes() {
            assert!(
                inputs.iter().any(|(axis, _)| axis == &required_axis),
                "spline graph node must bind required axis '{}'",
                required_axis.as_str()
            );
        }
        for (_, node) in &inputs {
            self.assert_existing_node(*node, "spline input");
        }
        self.push(Node::Spline {
            spline: Box::new(spline),
            inputs,
        })
    }

    pub(crate) fn set_channel(&mut self, channel: Channel, node: NodeId) {
        self.assert_existing_node(node, "channel output");
        self.outputs.insert(channel.0, node);
    }

    #[cfg(test)]
    pub(crate) fn has_channel(&self, channel: impl AsRef<str>) -> bool {
        self.outputs.contains_key(channel.as_ref())
    }

    #[cfg(test)]
    pub(crate) fn channel_names(&self) -> impl Iterator<Item = &str> {
        self.outputs.keys().map(String::as_str)
    }

    pub(crate) fn channel_node(&self, channel: impl AsRef<str>) -> Option<NodeId> {
        self.outputs.get(channel.as_ref()).copied()
    }

    pub(crate) fn node_depends_on_y(&self, node: NodeId) -> bool {
        self.assert_existing_node(node, "Y-dependency query root");
        self.y_dependencies[node.index]
    }

    #[cfg(test)]
    pub(crate) fn channel_depends_on_y(&self, channel: impl AsRef<str>) -> Option<bool> {
        self.channel_node(channel)
            .map(|node| self.node_depends_on_y(node))
    }

    pub(crate) fn evaluate_channel(
        &self,
        channel: impl AsRef<str>,
        point: SamplePoint,
    ) -> Option<f64> {
        self.channel_node(channel)
            .map(|node| self.evaluate_node(node, point))
    }

    pub(crate) fn evaluation_cache(&self) -> GraphEvaluationCache {
        GraphEvaluationCache::new(self)
    }

    pub(crate) fn evaluate_node(&self, node: NodeId, point: SamplePoint) -> f64 {
        self.assert_existing_node(node, "evaluation root");
        self.evaluate_node_uncached(node, point)
    }

    pub(crate) fn evaluate_node_cached(
        &self,
        node: NodeId,
        point: SamplePoint,
        cache: &mut GraphEvaluationCache,
    ) -> f64 {
        self.assert_existing_node(node, "evaluation root");
        cache.begin_sample(self);
        self.evaluate_node_cached_inner(node, point, cache)
    }

    fn evaluate_node_uncached(&self, node: NodeId, point: SamplePoint) -> f64 {
        match &self.nodes[node.index] {
            Node::Constant(value) => *value,
            #[cfg(test)]
            Node::Axis(Axis::X) => point.x,
            #[cfg(test)]
            Node::Axis(Axis::Y) => point.y,
            #[cfg(test)]
            Node::Axis(Axis::Z) => point.z,
            Node::SampledField(field) => field.sample(point),
            Node::Add(a, b) => {
                self.evaluate_node_uncached(*a, point) + self.evaluate_node_uncached(*b, point)
            }
            Node::Multiply(a, b) => {
                self.evaluate_node_uncached(*a, point) * self.evaluate_node_uncached(*b, point)
            }
            #[cfg(test)]
            Node::Min(a, b) => self
                .evaluate_node_uncached(*a, point)
                .min(self.evaluate_node_uncached(*b, point)),
            #[cfg(test)]
            Node::Max(a, b) => self
                .evaluate_node_uncached(*a, point)
                .max(self.evaluate_node_uncached(*b, point)),
            #[cfg(test)]
            Node::Abs(input) => self.evaluate_node_uncached(*input, point).abs(),
            Node::RidgeFold(input) => ridge_fold_value(self.evaluate_node_uncached(*input, point)),
            #[cfg(test)]
            Node::Clamp { input, min, max } => {
                let lo = (*min).min(*max);
                let hi = (*min).max(*max);
                self.evaluate_node_uncached(*input, point).clamp(lo, hi)
            }
            #[cfg(test)]
            Node::Lerp { a, b, t } => {
                let a = self.evaluate_node_uncached(*a, point);
                let b = self.evaluate_node_uncached(*b, point);
                a + (b - a) * self.evaluate_node_uncached(*t, point)
            }
            #[cfg(test)]
            Node::VerticalRamp { y_min, y_max } => vertical_ramp(point.y, *y_min, *y_max),
            Node::VerticalBias { base_height } => {
                self.evaluate_node_uncached(*base_height, point) - point.y
            }
            Node::FloorClamp {
                input,
                floor_y,
                fade_height,
                solid_density,
            } => floor_clamp_value(
                self.evaluate_node_uncached(*input, point),
                point.y,
                *floor_y,
                *fade_height,
                *solid_density,
            ),
            #[cfg(test)]
            Node::RangeSelect {
                selector,
                min,
                max,
                inside,
                outside,
            } => {
                let value = self.evaluate_node_uncached(*selector, point);
                let lo = (*min).min(*max);
                let hi = (*min).max(*max);
                if (lo..=hi).contains(&value) {
                    self.evaluate_node_uncached(*inside, point)
                } else {
                    self.evaluate_node_uncached(*outside, point)
                }
            }
            Node::Spline { spline, inputs } => {
                let mut spline_input = |axis: &SplineAxis| {
                    let node = inputs
                        .iter()
                        .find_map(|(input_axis, input_node)| {
                            (input_axis == axis).then_some(*input_node)
                        })
                        .unwrap_or_else(|| {
                            panic!("missing graph input for spline axis '{}'", axis.as_str())
                        });
                    self.evaluate_node_uncached(node, point)
                };
                spline.evaluate(&mut spline_input)
            }
        }
    }

    fn evaluate_node_cached_inner(
        &self,
        node: NodeId,
        point: SamplePoint,
        cache: &mut GraphEvaluationCache,
    ) -> f64 {
        let y_invariant = !self.y_dependencies[node.index];
        if let Some(value) = cache.get(node, y_invariant) {
            return value;
        }

        let value = match &self.nodes[node.index] {
            Node::Constant(value) => *value,
            #[cfg(test)]
            Node::Axis(Axis::X) => point.x,
            #[cfg(test)]
            Node::Axis(Axis::Y) => point.y,
            #[cfg(test)]
            Node::Axis(Axis::Z) => point.z,
            Node::SampledField(field) => field.sample(point),
            Node::Add(a, b) => {
                self.evaluate_node_cached_inner(*a, point, cache)
                    + self.evaluate_node_cached_inner(*b, point, cache)
            }
            Node::Multiply(a, b) => {
                self.evaluate_node_cached_inner(*a, point, cache)
                    * self.evaluate_node_cached_inner(*b, point, cache)
            }
            #[cfg(test)]
            Node::Min(a, b) => self
                .evaluate_node_cached_inner(*a, point, cache)
                .min(self.evaluate_node_cached_inner(*b, point, cache)),
            #[cfg(test)]
            Node::Max(a, b) => self
                .evaluate_node_cached_inner(*a, point, cache)
                .max(self.evaluate_node_cached_inner(*b, point, cache)),
            #[cfg(test)]
            Node::Abs(input) => self.evaluate_node_cached_inner(*input, point, cache).abs(),
            Node::RidgeFold(input) => {
                ridge_fold_value(self.evaluate_node_cached_inner(*input, point, cache))
            }
            #[cfg(test)]
            Node::Clamp { input, min, max } => {
                let lo = (*min).min(*max);
                let hi = (*min).max(*max);
                self.evaluate_node_cached_inner(*input, point, cache)
                    .clamp(lo, hi)
            }
            #[cfg(test)]
            Node::Lerp { a, b, t } => {
                let a = self.evaluate_node_cached_inner(*a, point, cache);
                let b = self.evaluate_node_cached_inner(*b, point, cache);
                a + (b - a) * self.evaluate_node_cached_inner(*t, point, cache)
            }
            #[cfg(test)]
            Node::VerticalRamp { y_min, y_max } => vertical_ramp(point.y, *y_min, *y_max),
            Node::VerticalBias { base_height } => {
                self.evaluate_node_cached_inner(*base_height, point, cache) - point.y
            }
            Node::FloorClamp {
                input,
                floor_y,
                fade_height,
                solid_density,
            } => floor_clamp_value(
                self.evaluate_node_cached_inner(*input, point, cache),
                point.y,
                *floor_y,
                *fade_height,
                *solid_density,
            ),
            #[cfg(test)]
            Node::RangeSelect {
                selector,
                min,
                max,
                inside,
                outside,
            } => {
                let value = self.evaluate_node_cached_inner(*selector, point, cache);
                let lo = (*min).min(*max);
                let hi = (*min).max(*max);
                if (lo..=hi).contains(&value) {
                    self.evaluate_node_cached_inner(*inside, point, cache)
                } else {
                    self.evaluate_node_cached_inner(*outside, point, cache)
                }
            }
            Node::Spline { spline, inputs } => {
                let mut spline_input = |axis: &SplineAxis| {
                    let node = inputs
                        .iter()
                        .find_map(|(input_axis, input_node)| {
                            (input_axis == axis).then_some(*input_node)
                        })
                        .unwrap_or_else(|| {
                            panic!("missing graph input for spline axis '{}'", axis.as_str())
                        });
                    self.evaluate_node_cached_inner(node, point, cache)
                };
                spline.evaluate(&mut spline_input)
            }
        };
        cache.store(node, value, y_invariant)
    }

    fn push(&mut self, node: Node) -> NodeId {
        let id = NodeId::new(self.graph_id, self.nodes.len());
        let depends_on_y = self.node_definition_depends_on_y(&node);
        self.nodes.push(node);
        self.y_dependencies.push(depends_on_y);
        id
    }

    fn node_definition_depends_on_y(&self, node: &Node) -> bool {
        match node {
            Node::Constant(_) => false,
            #[cfg(test)]
            Node::Axis(axis) => *axis == Axis::Y,
            Node::SampledField(field) => field.depends_on_y(),
            Node::Add(a, b) | Node::Multiply(a, b) => {
                self.node_depends_on_y(*a) || self.node_depends_on_y(*b)
            }
            #[cfg(test)]
            Node::Min(a, b) | Node::Max(a, b) => {
                self.node_depends_on_y(*a) || self.node_depends_on_y(*b)
            }
            Node::RidgeFold(input) => self.node_depends_on_y(*input),
            #[cfg(test)]
            Node::Abs(input) | Node::Clamp { input, .. } => self.node_depends_on_y(*input),
            #[cfg(test)]
            Node::Lerp { a, b, t } => {
                self.node_depends_on_y(*a)
                    || self.node_depends_on_y(*b)
                    || self.node_depends_on_y(*t)
            }
            #[cfg(test)]
            Node::VerticalRamp { .. } | Node::VerticalBias { .. } | Node::FloorClamp { .. } => true,
            #[cfg(not(test))]
            Node::VerticalBias { .. } | Node::FloorClamp { .. } => true,
            #[cfg(test)]
            Node::RangeSelect {
                selector,
                inside,
                outside,
                ..
            } => {
                self.node_depends_on_y(*selector)
                    || self.node_depends_on_y(*inside)
                    || self.node_depends_on_y(*outside)
            }
            Node::Spline { inputs, .. } => inputs
                .iter()
                .any(|(_, input)| self.node_depends_on_y(*input)),
        }
    }

    fn assert_existing_node(&self, node: NodeId, context: &str) {
        assert_eq!(
            node.graph_id, self.graph_id,
            "{context} belongs to a different scalar graph"
        );
        assert!(
            node.index < self.nodes.len(),
            "{context} must reference an existing scalar graph node"
        );
    }
}

fn advance_generation(generation: &mut u32, stamps: &mut [u32]) {
    if *generation == u32::MAX {
        stamps.fill(0);
        *generation = 1;
    } else {
        *generation += 1;
    }
}

#[cfg(test)]
fn vertical_ramp(y: f64, y_min: f64, y_max: f64) -> f64 {
    if (y_max - y_min).abs() <= f64::EPSILON {
        if y >= y_max {
            1.0
        } else {
            0.0
        }
    } else {
        ((y - y_min) / (y_max - y_min)).clamp(0.0, 1.0)
    }
}

pub(crate) fn ridge_fold_value(variance: f64) -> f64 {
    1.0 - ((3.0 * variance.abs()) - 2.0).abs()
}

fn floor_clamp_value(
    input: f64,
    y: f64,
    floor_y: f64,
    fade_height: f64,
    solid_density: f64,
) -> f64 {
    if fade_height <= f64::EPSILON {
        if y <= floor_y {
            solid_density
        } else {
            input
        }
    } else if y <= floor_y {
        solid_density
    } else if y < floor_y + fade_height {
        let t = ((y - floor_y) / fade_height).clamp(0.0, 1.0);
        solid_density + (input - solid_density) * t
    } else {
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Clone)]
    struct LinearField;

    impl fmt::Debug for LinearField {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("LinearField").finish()
        }
    }

    impl SampledScalarField for LinearField {
        fn sample(&self, point: SamplePoint) -> f64 {
            point.x + point.y - point.z
        }
    }

    #[derive(Clone)]
    struct CountingField {
        samples: Arc<AtomicUsize>,
    }

    impl fmt::Debug for CountingField {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("CountingField").finish()
        }
    }

    impl SampledScalarField for CountingField {
        fn sample(&self, point: SamplePoint) -> f64 {
            self.samples.fetch_add(1, Ordering::Relaxed);
            point.x + point.y * 2.0 - point.z
        }
    }

    #[derive(Clone)]
    struct HorizontalCountingField {
        samples: Arc<AtomicUsize>,
    }

    impl fmt::Debug for HorizontalCountingField {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("HorizontalCountingField").finish()
        }
    }

    impl SampledScalarField for HorizontalCountingField {
        fn sample(&self, point: SamplePoint) -> f64 {
            self.samples.fetch_add(1, Ordering::Relaxed);
            point.x - point.z
        }

        fn depends_on_y(&self) -> bool {
            false
        }
    }

    struct PanicField(&'static str);

    impl fmt::Debug for PanicField {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("PanicField").field(&self.0).finish()
        }
    }

    impl SampledScalarField for PanicField {
        fn sample(&self, _point: SamplePoint) -> f64 {
            panic!("unexpectedly evaluated {}", self.0)
        }
    }

    struct HorizontalPanicField(&'static str);

    impl fmt::Debug for HorizontalPanicField {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("HorizontalPanicField")
                .field(&self.0)
                .finish()
        }
    }

    impl SampledScalarField for HorizontalPanicField {
        fn sample(&self, _point: SamplePoint) -> f64 {
            panic!("unexpectedly evaluated {}", self.0)
        }

        fn depends_on_y(&self) -> bool {
            false
        }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn evaluates_scalar_nodes_at_sample_point() {
        let mut graph = ScalarGraph::new();
        let x = graph.axis(Axis::X);
        let y = graph.axis(Axis::Y);
        let z = graph.axis(Axis::Z);
        let two = graph.constant(2.0);
        let shifted_x = graph.add(x, two);
        let product = graph.multiply(shifted_x, z);
        let distance = graph.abs(product);
        let ramp = graph.vertical_ramp(0.0, 10.0);
        let lerped = graph.lerp(y, distance, ramp);
        let clamped = graph.clamp(lerped, -4.0, 4.0);
        let three = graph.constant(3.0);
        let minus_three = graph.constant(-3.0);
        let capped = graph.min(clamped, three);
        let output = graph.max(capped, minus_three);

        let point = SamplePoint::new(3.0, 5.0, -2.0);

        assert_close(graph.evaluate_node(output, point), 3.0);
    }

    #[test]
    fn cached_direct_evaluation_matches_channel_and_reuses_shared_nodes() {
        let samples = Arc::new(AtomicUsize::new(0));
        let mut graph = ScalarGraph::new();
        let shared = graph.sampled_field(CountingField {
            samples: samples.clone(),
        });
        let doubled = graph.add(shared, shared);
        let output = graph.multiply(doubled, doubled);
        graph.set_channel(Channel::new("terrain/cached"), output);

        let point = SamplePoint::new(3.0, 5.0, -2.0);
        let expected = graph.evaluate_channel("terrain/cached", point).unwrap();
        let mut cache = graph.evaluation_cache();

        samples.store(0, Ordering::Relaxed);
        let actual = graph.evaluate_node_cached(
            graph.channel_node("terrain/cached").unwrap(),
            point,
            &mut cache,
        );
        assert_close(actual, expected);
        assert_eq!(
            samples.load(Ordering::Relaxed),
            1,
            "cached evaluation should sample a shared field once per point"
        );

        samples.store(0, Ordering::Relaxed);
        let other = graph.evaluate_node_cached(
            graph.channel_node("terrain/cached").unwrap(),
            SamplePoint::new(4.0, 5.0, -2.0),
            &mut cache,
        );
        assert!(other.is_finite());
        assert_eq!(
            samples.load(Ordering::Relaxed),
            1,
            "cache state must not leak across sample points"
        );
    }

    #[test]
    fn y_dependency_metadata_is_composed_from_graph_inputs() {
        let samples = Arc::new(AtomicUsize::new(0));
        let mut graph = ScalarGraph::new();
        let constant = graph.constant(1.0);
        let x = graph.axis(Axis::X);
        let y = graph.axis(Axis::Y);
        let z = graph.axis(Axis::Z);
        let horizontal_field = graph.sampled_field(HorizontalCountingField {
            samples: samples.clone(),
        });
        let sampled_field = graph.sampled_field(CountingField { samples });
        let horizontal_sum = graph.add(x, z);
        let vertical_sum = graph.add(horizontal_sum, y);
        let horizontal_selector =
            graph.range_select(x, -1.0, 1.0, horizontal_field, horizontal_sum);
        let conservative_selector =
            graph.range_select(x, -1.0, 1.0, horizontal_field, sampled_field);
        let vertical_ramp = graph.vertical_ramp(0.0, 10.0);
        let vertical_bias = graph.vertical_bias(constant);
        let floor_clamp = graph.floor_clamp(horizontal_sum, 0.0, 4.0, 64.0);

        assert!(!graph.node_depends_on_y(constant));
        assert!(!graph.node_depends_on_y(x));
        assert!(!graph.node_depends_on_y(z));
        assert!(graph.node_depends_on_y(y));
        assert!(!graph.node_depends_on_y(horizontal_field));
        assert!(graph.node_depends_on_y(sampled_field));
        assert!(!graph.node_depends_on_y(horizontal_sum));
        assert!(graph.node_depends_on_y(vertical_sum));
        assert!(!graph.node_depends_on_y(horizontal_selector));
        assert!(graph.node_depends_on_y(conservative_selector));
        assert!(graph.node_depends_on_y(vertical_ramp));
        assert!(graph.node_depends_on_y(vertical_bias));
        assert!(graph.node_depends_on_y(floor_clamp));
    }

    #[test]
    fn y_invariant_column_cache_reuses_nodes_only_within_column() {
        let samples = Arc::new(AtomicUsize::new(0));
        let mut graph = ScalarGraph::new();
        let horizontal = graph.sampled_field(HorizontalCountingField {
            samples: samples.clone(),
        });
        let y = graph.axis(Axis::Y);
        let output = graph.add(horizontal, y);
        let mut cache = graph.evaluation_cache();

        cache.begin_y_invariant_column(&graph);
        for wy in [0.0, 8.0, 16.0] {
            assert_close(
                graph.evaluate_node_cached(output, SamplePoint::new(4.0, wy, -2.0), &mut cache),
                6.0 + wy,
            );
        }
        assert_eq!(
            samples.load(Ordering::Relaxed),
            1,
            "horizontal node should be sampled once per explicit X/Z column"
        );

        cache.begin_y_invariant_column(&graph);
        assert_close(
            graph.evaluate_node_cached(output, SamplePoint::new(9.0, 0.0, -2.0), &mut cache),
            11.0,
        );
        assert_eq!(
            samples.load(Ordering::Relaxed),
            2,
            "column cache must not leak across X/Z columns"
        );
    }

    #[test]
    fn range_selector_branches_on_inclusive_bounds() {
        let mut graph = ScalarGraph::new();
        let x = graph.axis(Axis::X);
        let inside = graph.constant(7.0);
        let outside = graph.constant(-2.0);
        let selected = graph.range_select(x, -1.0, 1.0, inside, outside);

        assert_close(
            graph.evaluate_node(selected, SamplePoint::new(-1.0, 0.0, 0.0)),
            7.0,
        );
        assert_close(
            graph.evaluate_node(selected, SamplePoint::new(0.25, 0.0, 0.0)),
            7.0,
        );
        assert_close(
            graph.evaluate_node(selected, SamplePoint::new(1.5, 0.0, 0.0)),
            -2.0,
        );
    }

    #[test]
    fn y_invariant_column_cache_preserves_range_selector_laziness() {
        let mut graph = ScalarGraph::new();
        let x = graph.axis(Axis::X);
        let inside = graph.constant(7.0);
        let outside_panic = graph.sampled_field(HorizontalPanicField("outside branch"));
        let selected = graph.range_select(x, -1.0, 1.0, inside, outside_panic);

        assert!(!graph.node_depends_on_y(selected));

        let mut cache = graph.evaluation_cache();
        cache.begin_y_invariant_column(&graph);
        for wy in [0.0, 8.0, 16.0] {
            assert_close(
                graph.evaluate_node_cached(selected, SamplePoint::new(0.25, wy, 0.0), &mut cache),
                7.0,
            );
        }
    }

    #[test]
    fn cached_range_selector_evaluates_only_selected_branch() {
        let mut graph = ScalarGraph::new();
        let x = graph.axis(Axis::X);
        let inside = graph.constant(7.0);
        let outside_panic = graph.sampled_field(PanicField("outside branch"));
        let inside_selected = graph.range_select(x, -1.0, 1.0, inside, outside_panic);

        let inside_panic = graph.sampled_field(PanicField("inside branch"));
        let outside = graph.constant(-2.0);
        let outside_selected = graph.range_select(x, -1.0, 1.0, inside_panic, outside);

        let mut cache = graph.evaluation_cache();
        assert_close(
            graph.evaluate_node_cached(
                inside_selected,
                SamplePoint::new(0.25, 0.0, 0.0),
                &mut cache,
            ),
            7.0,
        );
        assert_close(
            graph.evaluate_node_cached(
                outside_selected,
                SamplePoint::new(1.5, 0.0, 0.0),
                &mut cache,
            ),
            -2.0,
        );
    }

    #[test]
    fn named_channels_lookup_outputs_by_stable_name() {
        let mut graph = ScalarGraph::new();
        let ramp = graph.vertical_ramp(64.0, 80.0);
        graph.set_channel(Channel::new("terrain/base_density"), ramp);

        let point = SamplePoint::new(0.0, 72.0, 0.0);

        assert_eq!(graph.channel_node("missing"), None);
        assert!(graph.has_channel("terrain/base_density"));
        assert_eq!(
            graph.channel_names().collect::<Vec<_>>(),
            ["terrain/base_density"]
        );
        assert_eq!(graph.channel_node("terrain/base_density"), Some(ramp));
        assert_close(
            graph
                .evaluate_channel(Channel::new("terrain/base_density"), point)
                .unwrap(),
            0.5,
        );
    }

    #[test]
    fn channel_outputs_can_be_replaced_by_name() {
        let mut graph = ScalarGraph::new();
        let first = graph.constant(1.0);
        let second = graph.constant(2.0);
        graph.set_channel(Channel::new("master_density"), first);
        graph.set_channel(Channel::new("master_density"), second);

        assert_eq!(graph.channel_node("master_density"), Some(second));
        assert_close(
            graph
                .evaluate_channel("master_density", SamplePoint::new(0.0, 0.0, 0.0))
                .unwrap(),
            2.0,
        );
    }

    #[test]
    fn sampled_field_spline_ridge_and_floor_nodes_remain_pure() {
        let mut graph = ScalarGraph::new();
        let sampled = graph.sampled_field(LinearField);
        let x = graph.axis(Axis::X);
        let folded = graph.ridge_fold(x);
        let base = graph.constant(8.0);
        let bias = graph.vertical_bias(base);
        let spline = CubicSpline::new(
            "ridge",
            [
                spline::SplinePoint::constant(-1.0, 0.0),
                spline::SplinePoint::constant(1.0, 2.0),
            ],
        );
        let shaped = graph.spline(spline, [(SplineAxis::new("ridge"), folded)]);
        let shaped_bias = graph.multiply(bias, shaped);
        let combined = graph.add(sampled, shaped_bias);
        let clamped = graph.floor_clamp(combined, 0.0, 4.0, 64.0);

        assert_close(
            graph.evaluate_node(folded, SamplePoint::new(0.0, 0.0, 0.0)),
            -1.0,
        );
        assert_close(
            graph.evaluate_node(folded, SamplePoint::new(2.0 / 3.0, 0.0, 0.0)),
            1.0,
        );
        assert_close(
            graph.evaluate_node(clamped, SamplePoint::new(1.0, 0.0, 3.0)),
            64.0,
        );
        assert!(graph
            .evaluate_node(clamped, SamplePoint::new(1.0, 8.0, 3.0))
            .is_finite());
    }

    #[test]
    fn rejects_foreign_or_not_yet_existing_node_ids() {
        fn assert_panics(action: impl FnOnce()) {
            assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err());
        }

        let mut graph = ScalarGraph::new();
        let existing = graph.constant(1.0);
        let mut other = ScalarGraph::new();
        let foreign = other.constant(2.0);

        assert_panics(|| {
            graph.add(existing, foreign);
        });
        assert_eq!(
            graph.nodes.len(),
            1,
            "rejected foreign inputs must not append a node"
        );

        let future = NodeId::new(graph.graph_id, graph.nodes.len());
        assert_panics(|| {
            graph.abs(future);
        });
        assert_panics(|| {
            graph.set_channel(Channel::new("bad/future"), future);
        });
    }

    #[test]
    fn spline_construction_rejects_missing_nested_axes() {
        let mut graph = ScalarGraph::new();
        let x = graph.axis(Axis::X);
        let spline = CubicSpline::new(
            "x",
            [
                spline::SplinePoint::nested(
                    -1.0,
                    CubicSpline::new(
                        "y",
                        [
                            spline::SplinePoint::constant(-1.0, 0.0),
                            spline::SplinePoint::constant(1.0, 1.0),
                        ],
                    ),
                ),
                spline::SplinePoint::constant(1.0, 2.0),
            ],
        );

        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            graph.spline(spline, [(SplineAxis::new("x"), x)]);
        }))
        .is_err());
        assert_eq!(
            graph.nodes.len(),
            1,
            "rejected spline must not append a graph node"
        );
    }
}
