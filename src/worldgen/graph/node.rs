use super::spline::{CubicSpline, SplineAxis};
#[cfg(test)]
use super::Axis;
use super::{GraphEvaluationCache, GraphId, NodeId, SamplePoint, SampledScalarField, ScalarGraph};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(super) enum Node {
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
    Abs(NodeId),
    RidgeFold(NodeId),
    /// Soft terracing of a height-domain input: steps of `step` blocks with a
    /// twice-sharpened smoothstep riser (cliffy treads, no hard discontinuity
    /// so the C1 lattice reconstruction stays artifact-free).
    Terrace {
        input: NodeId,
        step: f64,
    },
    Clamp {
        input: NodeId,
        min: f64,
        max: f64,
    },
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
    pub(super) fn with_graph(self, graph_id: GraphId) -> Self {
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
            Self::Abs(input) => Self::Abs(input.with_graph(graph_id)),
            Self::RidgeFold(input) => Self::RidgeFold(input.with_graph(graph_id)),
            Self::Terrace { input, step } => Self::Terrace {
                input: input.with_graph(graph_id),
                step,
            },
            Self::Clamp { input, min, max } => Self::Clamp {
                input: input.with_graph(graph_id),
                min,
                max,
            },
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

impl ScalarGraph {
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
            Node::Abs(input) => self.evaluate_node_uncached(*input, point).abs(),
            Node::RidgeFold(input) => ridge_fold_value(self.evaluate_node_uncached(*input, point)),
            Node::Terrace { input, step } => {
                terrace_value(self.evaluate_node_uncached(*input, point), *step)
            }
            Node::Clamp { input, min, max } => {
                let lo = (*min).min(*max);
                let hi = (*min).max(*max);
                self.evaluate_node_uncached(*input, point).clamp(lo, hi)
            }
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
            Node::Abs(input) => self.evaluate_node_cached_inner(*input, point, cache).abs(),
            Node::RidgeFold(input) => {
                ridge_fold_value(self.evaluate_node_cached_inner(*input, point, cache))
            }
            Node::Terrace { input, step } => {
                terrace_value(self.evaluate_node_cached_inner(*input, point, cache), *step)
            }
            Node::Clamp { input, min, max } => {
                let lo = (*min).min(*max);
                let hi = (*min).max(*max);
                self.evaluate_node_cached_inner(*input, point, cache)
                    .clamp(lo, hi)
            }
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

    pub(super) fn node_definition_depends_on_y(&self, node: &Node) -> bool {
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
            Node::RidgeFold(input) | Node::Terrace { input, .. } => self.node_depends_on_y(*input),
            Node::Abs(input) | Node::Clamp { input, .. } => self.node_depends_on_y(*input),
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

/// Terrace a height value into `step`-block treads: the riser is a smoothstep
/// applied twice, so treads are near-flat with steep (but C1) risers.
pub(super) fn terrace_value(height: f64, step: f64) -> f64 {
    let cell = (height / step).floor();
    let t = height / step - cell;
    let s = t * t * (3.0 - 2.0 * t);
    let s = s * s * (3.0 - 2.0 * s);
    (cell + s) * step
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
