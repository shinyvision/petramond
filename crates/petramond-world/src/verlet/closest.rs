//! Closest-point and distance queries between points, segments, triangles and boxes:
//! the pairings a surface contact decomposes into. Two convex shapes that do not
//! overlap are nearest at a vertex-facet or an edge-edge pair, so a triangle's
//! distance to a box is the minimum over exactly those pairs.

use petramond_math::math::Vec3;

const EPS: f32 = 1e-9;

#[inline]
pub fn closest_on_box(p: Vec3, min: Vec3, max: Vec3) -> Vec3 {
    p.clamp(min, max)
}

pub fn box_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    std::array::from_fn(|k| {
        Vec3::new(
            if k & 1 == 0 { min.x } else { max.x },
            if k & 2 == 0 { min.y } else { max.y },
            if k & 4 == 0 { min.z } else { max.z },
        )
    })
}

/// The 12 edges of a box as corner index pairs into [`box_corners`].
pub const BOX_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// The point of triangle `abc` nearest `p`, with its barycentric weights.
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, [f32; 3]) {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, [1.0 - v, v, 0.0]);
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (a + ab * v + ac * w, [1.0 - v - w, v, w])
}

/// Closest points between segments `p0p1` and `q0q1`: the parameters `s`, `t` along
/// each and the two points.
pub fn closest_segments(p0: Vec3, p1: Vec3, q0: Vec3, q1: Vec3) -> (f32, f32, Vec3, Vec3) {
    let (d1, d2, r) = (p1 - p0, q1 - q0, p0 - q0);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    let (s, t);
    if a <= EPS && e <= EPS {
        return (0.0, 0.0, p0, q0);
    }
    if a <= EPS {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let s0 = if denom > EPS {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                t = t0;
                s = s0;
            }
        }
    }
    (s, t, p0 + d1 * s, q0 + d2 * t)
}

/// Whether triangle `abc` overlaps the box (separating axis test).
pub fn triangle_overlaps_box(a: Vec3, b: Vec3, c: Vec3, min: Vec3, max: Vec3) -> bool {
    let centre = (min + max) * 0.5;
    let half = (max - min) * 0.5;
    let v = [a - centre, b - centre, c - centre];
    let e = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let separated = |axis: Vec3| {
        if axis.length_squared() <= EPS {
            return false;
        }
        let p = v.map(|x| x.dot(axis));
        let r = half.x * axis.x.abs() + half.y * axis.y.abs() + half.z * axis.z.abs();
        p.iter().copied().fold(f32::INFINITY, f32::min) > r
            || p.iter().copied().fold(f32::NEG_INFINITY, f32::max) < -r
    };
    for edge in e {
        for unit in [Vec3::X, Vec3::Y, Vec3::Z] {
            if separated(unit.cross(edge)) {
                return false;
            }
        }
    }
    for unit in [Vec3::X, Vec3::Y, Vec3::Z] {
        if separated(unit) {
            return false;
        }
    }
    !separated(e[0].cross(e[1]))
}

/// Exact distance from triangle `abc` to the box; 0 when they overlap. Box corners
/// and edges farther from the triangle's bounds than the best pair so far are skipped.
pub fn triangle_box_distance(a: Vec3, b: Vec3, c: Vec3, min: Vec3, max: Vec3) -> f32 {
    if triangle_overlaps_box(a, b, c, min, max) {
        return 0.0;
    }
    let mut best = f32::INFINITY;
    for p in [a, b, c] {
        best = best.min((p - closest_on_box(p, min, max)).length());
    }
    let (lo, hi) = (a.min(b).min(c), a.max(b).max(c));
    let corners = box_corners(min, max);
    for q in corners {
        if box_box_distance(lo, hi, q, q) < best {
            best = best.min((q - closest_on_triangle(q, a, b, c).0).length());
        }
    }
    for (i, j) in BOX_EDGES {
        let (q0, q1) = (corners[i], corners[j]);
        if box_box_distance(lo, hi, q0.min(q1), q0.max(q1)) >= best {
            continue;
        }
        for (p0, p1) in [(a, b), (b, c), (c, a)] {
            let (_, _, x, y) = closest_segments(p0, p1, q0, q1);
            best = best.min((x - y).length());
        }
    }
    best
}

/// Distance between two boxes; a lower bound for anything they contain.
#[inline]
pub fn box_box_distance(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> f32 {
    ((a_min - b_max).max(b_min - a_max))
        .max(Vec3::ZERO)
        .length()
}
