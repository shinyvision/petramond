use super::node::terrace_value;
use super::spline::{CubicSpline, SplineAxis};
use super::*;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Terracing a heightfield must stay monotone (a non-monotone height map
/// strands floating terrain) and must preserve tread-corner heights so the
/// overall elevation trend survives.
#[test]
fn terrace_value_is_monotone_and_tread_anchored() {
    let step = 8.0;
    let mut prev = terrace_value(-64.0, step);
    let mut h = -64.0 + 0.05;
    while h < 256.0 {
        let t = terrace_value(h, step);
        assert!(
            t >= prev - 1e-12,
            "terrace must be monotone: f({h}) = {t} < previous {prev}"
        );
        prev = t;
        h += 0.05;
    }
    // Exact tread anchors: multiples of the step map to themselves.
    for k in -8..=32 {
        let anchor = k as f64 * step;
        assert!((terrace_value(anchor, step) - anchor).abs() < 1e-9);
    }
}

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
    let horizontal_selector = graph.range_select(x, -1.0, 1.0, horizontal_field, horizontal_sum);
    let conservative_selector = graph.range_select(x, -1.0, 1.0, horizontal_field, sampled_field);
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
