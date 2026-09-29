use std::collections::{BTreeMap, BTreeSet};

const INLINE_SPLINE_POINTS: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SplineAxis(String);

impl SplineAxis {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        assert!(!name.is_empty(), "spline axis names cannot be empty");
        Self(name)
    }

    pub fn as_str(&self) -> &str {
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

pub trait SplineInput {
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
pub struct CubicSpline {
    axis: SplineAxis,
    points: Vec<SplinePoint>,
    hermite: bool,
}

impl CubicSpline {
    pub fn new(axis: impl Into<SplineAxis>, points: impl Into<Vec<SplinePoint>>) -> Self {
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
        let hermite = points.iter().all(|point| point.derivative.is_some());
        Self {
            axis: axis.into(),
            points,
            hermite,
        }
    }

    #[cfg(test)]
    pub fn constant(axis: impl Into<SplineAxis>, value: f64) -> Self {
        Self::new(axis, [SplinePoint::constant(0.0, value)])
    }

    pub fn required_axes(&self) -> BTreeSet<SplineAxis> {
        let mut axes = BTreeSet::new();
        self.collect_required_axes(&mut axes);
        axes
    }

    /// Evaluates only the point values the result reads: the segment's ends,
    /// plus the neighbours its monotone slopes need. Nested splines at other
    /// points are never visited.
    pub fn evaluate<I: SplineInput + ?Sized>(&self, input: &mut I) -> f64 {
        let x = input.axis_value(&self.axis);
        let points = self.points.as_slice();
        let mut value = |index: usize| points[index].value.evaluate(&mut *input);
        let n = points.len();
        if n == 1 {
            return value(0);
        }
        let last = n - 1;
        if self.hermite {
            if x <= points[0].location {
                let slope = points[0].derivative.unwrap_or(0.0);
                return value(0) + slope * (x - points[0].location);
            }
            if x >= points[last].location {
                let slope = points[last].derivative.unwrap_or(0.0);
                return value(last) + slope * (x - points[last].location);
            }
            let segment = segment_of(points, x);
            let slopes = [
                points[segment].derivative.unwrap_or(0.0),
                points[segment + 1].derivative.unwrap_or(0.0),
            ];
            let ends = [value(segment), value(segment + 1)];
            return interpolate_hermite(points, ends, slopes, segment, x);
        }
        if x <= points[0].location {
            return value(0);
        }
        if x >= points[last].location {
            return value(last);
        }
        let segment = segment_of(points, x);
        let reads = |i: usize| match i {
            _ if n == 2 => 0..=1,
            0 => 0..=2,
            _ if i == last => last - 2..=last,
            _ => i - 1..=i + 1,
        };
        let (lo, hi) = (*reads(segment).start(), *reads(segment + 1).end());
        let mut values = [0.0; INLINE_SPLINE_POINTS];
        let mut spill = Vec::new();
        let values: &mut [f64] = if n <= INLINE_SPLINE_POINTS {
            &mut values[..n]
        } else {
            spill.resize(n, 0.0);
            &mut spill
        };
        for (i, slot) in values.iter_mut().enumerate().take(hi + 1).skip(lo) {
            *slot = value(i);
        }
        let slopes = [
            monotone_slope(points, values, segment),
            monotone_slope(points, values, segment + 1),
        ];
        interpolate_hermite(
            points,
            [values[segment], values[segment + 1]],
            slopes,
            segment,
            x,
        )
    }

    fn collect_required_axes(&self, axes: &mut BTreeSet<SplineAxis>) {
        axes.insert(self.axis.clone());
        for point in &self.points {
            point.value.collect_required_axes(axes);
        }
    }
}

#[derive(Clone, Debug)]
pub struct SplinePoint {
    location: f64,
    value: SplineValue,
    derivative: Option<f64>,
}

impl SplinePoint {
    #[cfg(test)]
    pub fn new(location: f64, value: SplineValue) -> Self {
        Self::with_optional_derivative(location, value, None)
    }

    pub fn with_optional_derivative(
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

    #[cfg(test)]
    pub fn constant(location: f64, value: f64) -> Self {
        Self::new(location, SplineValue::Constant(value))
    }

    #[cfg(test)]
    pub fn nested(location: f64, spline: CubicSpline) -> Self {
        Self::new(location, SplineValue::Spline(Box::new(spline)))
    }

    pub fn constant_with_derivative(location: f64, value: f64, derivative: f64) -> Self {
        Self::with_optional_derivative(location, SplineValue::Constant(value), Some(derivative))
    }

    pub fn nested_with_derivative(location: f64, spline: CubicSpline, derivative: f64) -> Self {
        Self::with_optional_derivative(
            location,
            SplineValue::Spline(Box::new(spline)),
            Some(derivative),
        )
    }
}

#[derive(Clone, Debug)]
pub enum SplineValue {
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

fn segment_of(points: &[SplinePoint], x: f64) -> usize {
    points
        .windows(2)
        .position(|pair| x >= pair[0].location && x <= pair[1].location)
        .expect("clamped spline coordinate must fall inside one segment")
}

/// The monotone (Fritsch–Carlson style) slope at `i`; reads only the values
/// [`CubicSpline::evaluate`] computed for it.
fn monotone_slope(points: &[SplinePoint], values: &[f64], i: usize) -> f64 {
    let n = points.len();
    if n == 2 {
        return secant(points, values, 0);
    }
    if i == 0 {
        return endpoint_slope(
            span(points, 0),
            span(points, 1),
            secant(points, values, 0),
            secant(points, values, 1),
        );
    }
    if i == n - 1 {
        return endpoint_slope(
            span(points, n - 2),
            span(points, n - 3),
            secant(points, values, n - 2),
            secant(points, values, n - 3),
        );
    }
    let h_prev = span(points, i - 1);
    let h_next = span(points, i);
    let d_prev = secant(points, values, i - 1);
    let d_next = secant(points, values, i);
    if d_prev * d_next <= 0.0 {
        0.0
    } else {
        let w1 = 2.0 * h_next + h_prev;
        let w2 = h_next + 2.0 * h_prev;
        (w1 + w2) / (w1 / d_prev + w2 / d_next)
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

fn interpolate_hermite(
    points: &[SplinePoint],
    [v0, v1]: [f64; 2],
    [s0, s1]: [f64; 2],
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

    h00 * v0 + h10 * h * s0 + h01 * v1 + h11 * h * s1
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
