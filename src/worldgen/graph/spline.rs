//! Data-declared cubic splines for pure scalar graph evaluation.

#![allow(dead_code)] // Stage-3 infrastructure is wired into live terrain later.

use std::collections::{BTreeMap, BTreeSet};

const INLINE_SPLINE_POINTS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SplineAxis(String);

impl SplineAxis {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        assert!(!name.is_empty(), "spline axis names cannot be empty");
        Self(name)
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SplineAxis {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for SplineAxis {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

pub(crate) trait SplineInput {
    fn axis_value(&mut self, axis: &SplineAxis) -> f64;
}

impl<F> SplineInput for F
where
    F: for<'a> FnMut(&'a SplineAxis) -> f64,
{
    fn axis_value(&mut self, axis: &SplineAxis) -> f64 {
        self(axis)
    }
}

impl SplineInput for BTreeMap<SplineAxis, f64> {
    fn axis_value(&mut self, axis: &SplineAxis) -> f64 {
        *self
            .get(axis)
            .unwrap_or_else(|| panic!("missing spline input axis '{}'", axis.as_str()))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CubicSpline {
    axis: SplineAxis,
    points: Vec<SplinePoint>,
}

impl CubicSpline {
    pub(crate) fn new(axis: impl Into<SplineAxis>, points: impl Into<Vec<SplinePoint>>) -> Self {
        let points = points.into();
        assert!(!points.is_empty(), "cubic splines need at least one point");
        for point in &points {
            assert!(
                point.location.is_finite(),
                "spline point locations must be finite"
            );
        }
        for pair in points.windows(2) {
            assert!(
                pair[0].location < pair[1].location,
                "spline point locations must be strictly increasing"
            );
        }
        Self {
            axis: axis.into(),
            points,
        }
    }

    pub(crate) fn constant(axis: impl Into<SplineAxis>, value: f64) -> Self {
        Self::new(axis, [SplinePoint::constant(0.0, value)])
    }

    pub(crate) fn axis(&self) -> &SplineAxis {
        &self.axis
    }

    pub(crate) fn points(&self) -> &[SplinePoint] {
        &self.points
    }

    pub(crate) fn required_axes(&self) -> BTreeSet<SplineAxis> {
        let mut axes = BTreeSet::new();
        self.collect_required_axes(&mut axes);
        axes
    }

    pub(crate) fn evaluate<I: SplineInput + ?Sized>(&self, input: &mut I) -> f64 {
        let x = input.axis_value(&self.axis);
        let point_count = self.points.len();
        if point_count <= INLINE_SPLINE_POINTS {
            let mut values = [0.0; INLINE_SPLINE_POINTS];
            for (index, point) in self.points.iter().enumerate() {
                values[index] = point.value.evaluate(input);
            }
            evaluate_points(self.points.as_slice(), &values[..point_count], x)
        } else {
            let mut values = Vec::with_capacity(point_count);
            for point in &self.points {
                values.push(point.value.evaluate(input));
            }
            evaluate_points(self.points.as_slice(), values.as_slice(), x)
        }
    }

    fn collect_required_axes(&self, axes: &mut BTreeSet<SplineAxis>) {
        axes.insert(self.axis.clone());
        for point in &self.points {
            point.value.collect_required_axes(axes);
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SplinePoint {
    location: f64,
    value: SplineValue,
    /// Explicit Hermite tangent at this knot. When every point of a spline
    /// declares one, the spline uses cubic-Hermite interpolation with these exact
    /// tangents and linear extrapolation past the ends. When none do, the spline
    /// auto-derives monotone slopes and clamps past the ends instead.
    derivative: Option<f64>,
}

impl SplinePoint {
    pub(crate) fn new(location: f64, value: SplineValue) -> Self {
        Self::with_optional_derivative(location, value, None)
    }

    fn with_optional_derivative(
        location: f64,
        value: SplineValue,
        derivative: Option<f64>,
    ) -> Self {
        assert!(
            location.is_finite(),
            "spline point locations must be finite"
        );
        Self {
            location,
            value,
            derivative,
        }
    }

    pub(crate) fn constant(location: f64, value: f64) -> Self {
        Self::new(location, SplineValue::Constant(value))
    }

    pub(crate) fn nested(location: f64, spline: CubicSpline) -> Self {
        Self::new(location, SplineValue::Spline(Box::new(spline)))
    }

    pub(crate) fn constant_with_derivative(location: f64, value: f64, derivative: f64) -> Self {
        Self::with_optional_derivative(location, SplineValue::Constant(value), Some(derivative))
    }

    pub(crate) fn nested_with_derivative(
        location: f64,
        spline: CubicSpline,
        derivative: f64,
    ) -> Self {
        Self::with_optional_derivative(
            location,
            SplineValue::Spline(Box::new(spline)),
            Some(derivative),
        )
    }

    pub(crate) fn location(&self) -> f64 {
        self.location
    }

    pub(crate) fn value(&self) -> &SplineValue {
        &self.value
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SplineValue {
    Constant(f64),
    Spline(Box<CubicSpline>),
}

impl SplineValue {
    fn evaluate<I: SplineInput + ?Sized>(&self, input: &mut I) -> f64 {
        match self {
            Self::Constant(value) => *value,
            Self::Spline(spline) => spline.evaluate(input),
        }
    }

    fn collect_required_axes(&self, axes: &mut BTreeSet<SplineAxis>) {
        match self {
            Self::Constant(_) => {}
            Self::Spline(spline) => spline.collect_required_axes(axes),
        }
    }
}

fn evaluate_points(points: &[SplinePoint], values: &[f64], x: f64) -> f64 {
    debug_assert_eq!(points.len(), values.len());
    if points.len() == 1 {
        return values[0];
    }
    if points.iter().all(|point| point.derivative.is_some()) {
        return evaluate_hermite_explicit(points, values, x);
    }

    if x <= points[0].location {
        return values[0];
    }
    let last = points.len() - 1;
    if x >= points[last].location {
        return values[last];
    }

    let segment = points
        .windows(2)
        .position(|pair| x >= pair[0].location && x <= pair[1].location)
        .expect("clamped spline coordinate must fall inside one segment");
    if points.len() <= INLINE_SPLINE_POINTS {
        let mut slopes = [0.0; INLINE_SPLINE_POINTS];
        fill_monotone_slopes(points, values, &mut slopes[..points.len()]);
        interpolate_segment(points, values, &slopes[..points.len()], segment, x)
    } else {
        let mut slopes = vec![0.0; points.len()];
        fill_monotone_slopes(points, values, slopes.as_mut_slice());
        interpolate_segment(points, values, slopes.as_slice(), segment, x)
    }
}

/// Cubic-Hermite evaluation with the knots' explicit tangents (matching the
/// reference terrain shaper): linear extrapolation past either end, cubic Hermite
/// inside. `interpolate_segment` already applies Hermite given per-knot slopes, so
/// here the declared derivatives are fed in directly.
fn evaluate_hermite_explicit(points: &[SplinePoint], values: &[f64], x: f64) -> f64 {
    let last = points.len() - 1;
    if x <= points[0].location {
        let slope = points[0].derivative.unwrap_or(0.0);
        return values[0] + slope * (x - points[0].location);
    }
    if x >= points[last].location {
        let slope = points[last].derivative.unwrap_or(0.0);
        return values[last] + slope * (x - points[last].location);
    }

    let segment = points
        .windows(2)
        .position(|pair| x >= pair[0].location && x <= pair[1].location)
        .expect("clamped spline coordinate must fall inside one segment");
    let slopes = [
        points[segment].derivative.unwrap_or(0.0),
        points[segment + 1].derivative.unwrap_or(0.0),
    ];
    interpolate_segment_pair(points, values, &slopes, segment, x)
}

fn fill_monotone_slopes(points: &[SplinePoint], values: &[f64], slopes: &mut [f64]) {
    let n = points.len();
    debug_assert_eq!(points.len(), values.len());
    debug_assert_eq!(points.len(), slopes.len());
    if n == 2 {
        let secant = secant(points, values, 0);
        slopes[0] = secant;
        slopes[1] = secant;
        return;
    }

    slopes[0] = endpoint_slope(
        span(points, 0),
        span(points, 1),
        secant(points, values, 0),
        secant(points, values, 1),
    );
    slopes[n - 1] = endpoint_slope(
        span(points, n - 2),
        span(points, n - 3),
        secant(points, values, n - 2),
        secant(points, values, n - 3),
    );
    for i in 1..n - 1 {
        let h_prev = span(points, i - 1);
        let h_next = span(points, i);
        let d_prev = secant(points, values, i - 1);
        let d_next = secant(points, values, i);
        slopes[i] = if d_prev * d_next <= 0.0 {
            0.0
        } else {
            let w1 = 2.0 * h_next + h_prev;
            let w2 = h_next + 2.0 * h_prev;
            (w1 + w2) / (w1 / d_prev + w2 / d_next)
        };
    }
}

fn endpoint_slope(h0: f64, h1: f64, d0: f64, d1: f64) -> f64 {
    let slope = ((2.0 * h0 + h1) * d0 - h0 * d1) / (h0 + h1);
    if slope.signum() != d0.signum() {
        0.0
    } else if d0.signum() != d1.signum() && slope.abs() > 3.0 * d0.abs() {
        3.0 * d0
    } else {
        slope
    }
}

fn secant(points: &[SplinePoint], values: &[f64], index: usize) -> f64 {
    (values[index + 1] - values[index]) / (points[index + 1].location - points[index].location)
}

fn span(points: &[SplinePoint], index: usize) -> f64 {
    points[index + 1].location - points[index].location
}

fn interpolate_segment(
    points: &[SplinePoint],
    values: &[f64],
    slopes: &[f64],
    segment: usize,
    x: f64,
) -> f64 {
    let x0 = points[segment].location;
    let x1 = points[segment + 1].location;
    let h = x1 - x0;
    let t = (x - x0) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;

    h00 * values[segment]
        + h10 * h * slopes[segment]
        + h01 * values[segment + 1]
        + h11 * h * slopes[segment + 1]
}

/// Cubic Hermite over one segment using the two endpoint slopes directly
/// (`slopes[0]` at `segment`, `slopes[1]` at `segment + 1`).
fn interpolate_segment_pair(
    points: &[SplinePoint],
    values: &[f64],
    slopes: &[f64],
    segment: usize,
    x: f64,
) -> f64 {
    let x0 = points[segment].location;
    let x1 = points[segment + 1].location;
    let h = x1 - x0;
    let t = (x - x0) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;

    h00 * values[segment] + h10 * h * slopes[0] + h01 * values[segment + 1] + h11 * h * slopes[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn cubic_spline_interpolates_between_declared_points() {
        let spline = CubicSpline::new(
            "x",
            [
                SplinePoint::constant(0.0, 0.0),
                SplinePoint::constant(1.0, 10.0),
            ],
        );
        let mut input = |axis: &SplineAxis| {
            assert_eq!(axis.as_str(), "x");
            0.25
        };

        assert_close(spline.evaluate(&mut input), 2.5);
    }

    #[test]
    fn nested_splines_evaluate_on_their_own_axes() {
        let low_y = CubicSpline::new(
            "y",
            [
                SplinePoint::constant(-1.0, 0.0),
                SplinePoint::constant(1.0, 10.0),
            ],
        );
        let high_y = CubicSpline::new(
            "y",
            [
                SplinePoint::constant(-1.0, 20.0),
                SplinePoint::constant(1.0, 40.0),
            ],
        );
        let spline = CubicSpline::new(
            "x",
            [
                SplinePoint::nested(-1.0, low_y),
                SplinePoint::nested(1.0, high_y),
            ],
        );
        let mut input = BTreeMap::new();
        input.insert(SplineAxis::new("x"), 0.0);
        input.insert(SplineAxis::new("y"), 1.0);

        assert_close(spline.evaluate(&mut input), 25.0);
    }

    #[test]
    fn spline_clamps_outside_declared_domain() {
        let spline = CubicSpline::new(
            "x",
            [
                SplinePoint::constant(-1.0, -8.0),
                SplinePoint::constant(1.0, 12.0),
            ],
        );

        let mut below = |_axis: &SplineAxis| -4.0;
        let mut above = |_axis: &SplineAxis| 4.0;
        assert_close(spline.evaluate(&mut below), -8.0);
        assert_close(spline.evaluate(&mut above), 12.0);
    }

    #[test]
    fn explicit_derivatives_use_hermite_with_linear_extrapolation() {
        // Symmetric unit tangents with equal endpoints: Hermite stays at 0 at the
        // midpoint, and the ends extrapolate linearly along the tangent.
        let ramp = CubicSpline::new(
            "x",
            [
                SplinePoint::constant_with_derivative(-1.0, 0.0, 1.0),
                SplinePoint::constant_with_derivative(1.0, 0.0, 1.0),
            ],
        );
        let mut mid = |_: &SplineAxis| 0.0;
        let mut below = |_: &SplineAxis| -2.0;
        let mut above = |_: &SplineAxis| 3.0;
        assert_close(ramp.evaluate(&mut mid), 0.0);
        assert_close(ramp.evaluate(&mut below), -1.0);
        assert_close(ramp.evaluate(&mut above), 2.0);

        // Zero tangents reproduce the smoothstep midpoint (value 5 halfway 0→10),
        // distinct from the monotone path which would also pass through 5 here but
        // via auto-derived slopes.
        let step = CubicSpline::new(
            "x",
            [
                SplinePoint::constant_with_derivative(-1.0, 0.0, 0.0),
                SplinePoint::constant_with_derivative(1.0, 10.0, 0.0),
            ],
        );
        let mut center = |_: &SplineAxis| 0.0;
        assert_close(step.evaluate(&mut center), 5.0);
    }

    #[test]
    fn monotone_inputs_stay_sane_without_overshoot() {
        let spline = CubicSpline::new(
            "x",
            [
                SplinePoint::constant(-1.0, -10.0),
                SplinePoint::constant(-0.25, -2.0),
                SplinePoint::constant(0.5, 3.0),
                SplinePoint::constant(1.0, 9.0),
            ],
        );

        let mut first = |_axis: &SplineAxis| -1.0;
        let mut previous = spline.evaluate(&mut first);
        for step in 1..=32 {
            let x = -1.0 + step as f64 * (2.0 / 32.0);
            let mut input = |_axis: &SplineAxis| x;
            let value = spline.evaluate(&mut input);
            assert!(value >= previous, "monotone spline moved backward at {x}");
            assert!(
                (-10.0..=9.0).contains(&value),
                "monotone spline overshot declared value range: {value}"
            );
            previous = value;
        }
    }
}
