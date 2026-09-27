//! Continent height offset spline, nested cubic-Hermite over continentality, erosion and ridge.
//! It's the test reference the `offset` splines in `assets/density/terrain.json` were generated
//! from. The live recipe is data: [`crate::data::terrain`].
//!
//! [`offset_spline`] gives the offset. The surface sits where the vertical depth gradient cancels
//! it: `base_height ~= 128*(1 - 0.50375) + 128*offset` (see the density assembly in `terrain.rs`).
//! Built to match the reference land-spline exactly.
//!
//! We only need the depth-zero crossing. The reference's squash factor and peak jaggedness shape
//! caves and overhangs, which the surface-height model doesn't use, so they're not built here.

use crate::graph::spline::{CubicSpline, SplinePoint};

pub mod axes {
    pub const CONTINENTALITY: &str = "continentality";
    pub const EROSION: &str = "erosion";
    pub const RIDGE: &str = "ridge";
}

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

fn offset_value(variance: f64, continentality: f64) -> f64 {
    let f0 = 1.0 - (1.0 - continentality) * 0.5;
    let f1 = 0.5 * (1.0 - continentality);
    let f2 = (variance + 1.17) * 0.46082947;
    let off = f2 * f0 - f1;
    if variance < -0.7 {
        off.max(-0.2222)
    } else {
        off.max(0.0)
    }
}

fn ridge_offset_spline(f: f64, bl: bool) -> CubicSpline {
    let i = offset_value(-1.0, f);
    let k = offset_value(1.0, f);
    let l0 = 1.0 - (1.0 - f) * 0.5;
    let u0 = 0.5 * (1.0 - f);
    let l = u0 / (0.46082947 * l0) - 1.17;

    let points = if -0.65 < l && l < 1.0 {
        let u = offset_value(-0.65, f);
        let p = offset_value(-0.75, f);
        let q = (p - i) * 4.0;
        let r = offset_value(l, f);
        let s = (k - r) / (1.0 - l);
        vec![
            SplinePoint::constant_with_derivative(-1.0, i, q),
            SplinePoint::constant_with_derivative(-0.75, p, 0.0),
            SplinePoint::constant_with_derivative(-0.65, u, 0.0),
            SplinePoint::constant_with_derivative(l - 0.01, r, 0.0),
            SplinePoint::constant_with_derivative(l, r, s),
            SplinePoint::constant_with_derivative(1.0, k, s),
        ]
    } else {
        let u = (k - i) * 0.5;
        if bl {
            vec![
                SplinePoint::constant_with_derivative(-1.0, i.max(0.2), 0.0),
                SplinePoint::constant_with_derivative(0.0, lerp(0.5, i, k), u),
                SplinePoint::constant_with_derivative(1.0, k, u),
            ]
        } else {
            vec![
                SplinePoint::constant_with_derivative(-1.0, i, u),
                SplinePoint::constant_with_derivative(1.0, k, u),
            ]
        }
    };
    CubicSpline::new(axes::RIDGE, points)
}

fn flat_offset_spline(f: f64, g: f64, h: f64, i: f64, j: f64, k: f64) -> CubicSpline {
    let l = (0.5 * (g - f)).max(k);
    let m = 5.0 * (h - g);
    CubicSpline::new(
        axes::RIDGE,
        vec![
            SplinePoint::constant_with_derivative(-1.0, f, l),
            SplinePoint::constant_with_derivative(-0.4, g, l.min(m)),
            SplinePoint::constant_with_derivative(0.0, h, m),
            SplinePoint::constant_with_derivative(0.4, i, 2.0 * (i - h)),
            SplinePoint::constant_with_derivative(1.0, j, 0.7 * (j - i)),
        ],
    )
}

fn land_spline(f: f64, g: f64, h: f64, i: f64, j: f64, k: f64, bl: bool) -> CubicSpline {
    let sp1 = ridge_offset_spline(lerp(i, 0.6, 1.5), bl);
    let sp2 = ridge_offset_spline(lerp(i, 0.6, 1.0), bl);
    let sp3 = ridge_offset_spline(i, bl);
    let ih = 0.5 * i;
    let sp4 = flat_offset_spline(f - 0.15, ih, ih, ih, i * 0.6, 0.5);
    let sp5 = flat_offset_spline(f, j * i, g * i, ih, i * 0.6, 0.5);
    let sp6 = flat_offset_spline(f, j, j, g, h, 0.5);
    let sp8 = CubicSpline::new(
        axes::RIDGE,
        vec![
            SplinePoint::constant_with_derivative(-1.0, f, 0.0),
            SplinePoint::nested_with_derivative(-0.4, sp6.clone(), 0.0),
            SplinePoint::constant_with_derivative(0.0, h + 0.07, 0.0),
        ],
    );
    let sp9 = flat_offset_spline(-0.02, k, k, g, h, 0.0);

    let mut points = vec![
        SplinePoint::nested_with_derivative(-0.85, sp1, 0.0),
        SplinePoint::nested_with_derivative(-0.7, sp2, 0.0),
        SplinePoint::nested_with_derivative(-0.4, sp3, 0.0),
        SplinePoint::nested_with_derivative(-0.35, sp4, 0.0),
        SplinePoint::nested_with_derivative(-0.1, sp5, 0.0),
        SplinePoint::nested_with_derivative(0.2, sp6.clone(), 0.0),
    ];
    if bl {
        points.push(SplinePoint::nested_with_derivative(0.4, sp6.clone(), 0.0));
        points.push(SplinePoint::nested_with_derivative(0.45, sp8.clone(), 0.0));
        points.push(SplinePoint::nested_with_derivative(0.55, sp8, 0.0));
        points.push(SplinePoint::nested_with_derivative(0.58, sp6.clone(), 0.0));
    }
    points.push(SplinePoint::nested_with_derivative(0.7, sp9, 0.0));
    CubicSpline::new(axes::EROSION, points)
}

pub fn offset_spline() -> CubicSpline {
    let sp1 = land_spline(-0.15, 0.0, 0.0, 0.1, 0.0, -0.03, false);
    let sp2 = land_spline(-0.10, 0.03, 0.1, 0.1, 0.01, -0.03, false);
    let sp3 = land_spline(-0.10, 0.03, 0.1, 0.7, 0.01, -0.03, true);
    let sp4 = land_spline(-0.05, 0.03, 0.1, 1.0, 0.01, 0.01, true);
    CubicSpline::new(
        axes::CONTINENTALITY,
        vec![
            SplinePoint::constant_with_derivative(-1.10, 0.044, 0.0),
            SplinePoint::constant_with_derivative(-1.02, -0.2222, 0.0),
            SplinePoint::constant_with_derivative(-0.51, -0.2222, 0.0),
            SplinePoint::constant_with_derivative(-0.44, -0.12, 0.0),
            SplinePoint::constant_with_derivative(-0.18, -0.12, 0.0),
            SplinePoint::nested_with_derivative(-0.16, sp1.clone(), 0.0),
            SplinePoint::nested_with_derivative(-0.15, sp1, 0.0),
            SplinePoint::nested_with_derivative(-0.10, sp2, 0.0),
            SplinePoint::nested_with_derivative(0.25, sp3, 0.0),
            SplinePoint::nested_with_derivative(1.00, sp4, 0.0),
        ],
    )
}
