//! Evaluates graph roots over a batch of points, a node at a time: sampled
//! fields see the whole batch at once (and can share work across it), and the
//! per-node dispatch is paid once per batch instead of once per point.
//!
//! Every node computes exactly what the pointwise evaluator computes, so the
//! results are bit-identical. Unlike the pointwise evaluator, both branches of
//! a range select are evaluated for every point; nodes are pure functions of
//! the point, so this only costs time.

use super::node::{floor_clamp_value, ridge_fold_value, terrace_value, vertical_ramp, Node};
use super::spline::SplineAxis;
use super::{Axis, NodeId, SamplePoint, ScalarGraph};

impl ScalarGraph {
    pub fn evaluate_nodes_batch<const N: usize>(
        &self,
        roots: [NodeId; N],
        points: &[SamplePoint],
        out: &mut [[f64; N]],
    ) {
        assert_eq!(points.len(), out.len());
        for root in roots {
            self.assert_existing_node(root, "evaluation root");
        }
        let n = points.len();
        let mut needed = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = roots.iter().map(|r| r.index).collect();
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut needed[i], true) {
                continue;
            }
            match &self.nodes[i] {
                Node::Constant(_) | Node::Axis(_) | Node::SampledField(_) => {}
                Node::VerticalRamp { .. } => {}
                Node::Add(a, b) | Node::Multiply(a, b) | Node::Min(a, b) | Node::Max(a, b) => {
                    stack.extend([a.index, b.index]);
                }
                Node::Abs(input)
                | Node::RidgeFold(input)
                | Node::Terrace { input, .. }
                | Node::Clamp { input, .. }
                | Node::FloorClamp { input, .. } => stack.push(input.index),
                Node::VerticalBias { base_height } => stack.push(base_height.index),
                Node::Lerp { a, b, t } => stack.extend([a.index, b.index, t.index]),
                Node::RangeSelect {
                    selector,
                    inside,
                    outside,
                    ..
                } => stack.extend([selector.index, inside.index, outside.index]),
                Node::Spline { inputs, .. } => stack.extend(inputs.iter().map(|(_, n)| n.index)),
            }
        }
        let mut slot = vec![usize::MAX; self.nodes.len()];
        let mut values: Vec<f64> = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if !needed[i] {
                continue;
            }
            let base = values.len();
            values.resize(base + n, 0.0);
            let (done, own) = values.split_at_mut(base);
            let of = |id: &NodeId| &done[slot[id.index]..slot[id.index] + n];
            match node {
                Node::Constant(value) => own.fill(*value),
                Node::Axis(axis) => {
                    for (o, p) in own.iter_mut().zip(points) {
                        *o = match axis {
                            Axis::X => p.x,
                            Axis::Y => p.y,
                            Axis::Z => p.z,
                        };
                    }
                }
                Node::SampledField(field) => field.sample_batch(points, own),
                Node::Add(a, b) => zip2(own, of(a), of(b), |a, b| a + b),
                Node::Multiply(a, b) => zip2(own, of(a), of(b), |a, b| a * b),
                Node::Min(a, b) => zip2(own, of(a), of(b), f64::min),
                Node::Max(a, b) => zip2(own, of(a), of(b), f64::max),
                Node::Abs(input) => zip1(own, of(input), f64::abs),
                Node::RidgeFold(input) => zip1(own, of(input), ridge_fold_value),
                Node::Terrace { input, step } => zip1(own, of(input), |v| terrace_value(v, *step)),
                Node::Clamp { input, min, max } => {
                    let (lo, hi) = ((*min).min(*max), (*min).max(*max));
                    zip1(own, of(input), |v| v.clamp(lo, hi));
                }
                Node::Lerp { a, b, t } => {
                    let (a, b, t) = (of(a), of(b), of(t));
                    for k in 0..n {
                        own[k] = a[k] + (b[k] - a[k]) * t[k];
                    }
                }
                Node::VerticalRamp { y_min, y_max } => {
                    for (o, p) in own.iter_mut().zip(points) {
                        *o = vertical_ramp(p.y, *y_min, *y_max);
                    }
                }
                Node::VerticalBias { base_height } => {
                    for ((o, p), h) in own.iter_mut().zip(points).zip(of(base_height)) {
                        *o = h - p.y;
                    }
                }
                Node::FloorClamp {
                    input,
                    floor_y,
                    fade_height,
                    solid_density,
                } => {
                    for ((o, p), v) in own.iter_mut().zip(points).zip(of(input)) {
                        *o = floor_clamp_value(*v, p.y, *floor_y, *fade_height, *solid_density);
                    }
                }
                Node::RangeSelect {
                    selector,
                    min,
                    max,
                    inside,
                    outside,
                } => {
                    let (lo, hi) = ((*min).min(*max), (*min).max(*max));
                    let (s, a, b) = (of(selector), of(inside), of(outside));
                    for k in 0..n {
                        own[k] = if (lo..=hi).contains(&s[k]) {
                            a[k]
                        } else {
                            b[k]
                        };
                    }
                }
                Node::Spline { spline, inputs } => {
                    for (k, o) in own.iter_mut().enumerate() {
                        let mut input = |axis: &SplineAxis| {
                            let (_, node) = inputs
                                .iter()
                                .find(|(input_axis, _)| input_axis == axis)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "missing graph input for spline axis '{}'",
                                        axis.as_str()
                                    )
                                });
                            done[slot[node.index] + k]
                        };
                        *o = spline.evaluate(&mut input);
                    }
                }
            }
            slot[i] = base;
        }
        for (k, row) in out.iter_mut().enumerate() {
            for (value, root) in row.iter_mut().zip(roots) {
                *value = values[slot[root.index] + k];
            }
        }
    }
}

#[inline]
fn zip1(out: &mut [f64], a: &[f64], f: impl Fn(f64) -> f64) {
    for (o, &a) in out.iter_mut().zip(a) {
        *o = f(a);
    }
}

#[inline]
fn zip2(out: &mut [f64], a: &[f64], b: &[f64], f: impl Fn(f64, f64) -> f64) {
    for ((o, &a), &b) in out.iter_mut().zip(a).zip(b) {
        *o = f(a, b);
    }
}
