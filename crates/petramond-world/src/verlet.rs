//! Position-based particle physics shared by every soft body: Verlet integration,
//! distance constraints, and point-vs-voxel collision that walks every cell boundary a
//! move crosses.
//!
//! Callers own their particles and their constraint topology (a ragdoll's box corners,
//! a cloth's grid); this module only moves points. Collision asks `solid(cell)`, so the
//! caller decides which cells stop a point and in which frame cells are named.

use petramond_math::math::{voxel_at, IVec3, Vec3};

pub mod closest;

const FACE_EPS: f32 = 1e-3;
/// Longest single-axis move a sweep walks; a farther target is clamped, never skipped.
const MAX_SWEEP: f32 = 16.0;

/// One Verlet step: velocity is implied by `x - x_old`, scaled by `damp` (1.0 = none).
#[inline]
pub fn integrate(x: &mut Vec3, x_old: &mut Vec3, accel: Vec3, dt2: f32, damp: f32) {
    let vel = (*x - *x_old) * damp;
    let next = *x + vel + accel * dt2;
    *x_old = *x;
    *x = next;
}

/// Moves `a` and `b` toward `rest` apart, split by inverse masses `wa`/`wb` (0 = pinned).
/// `stiffness` in 0..=1 is the fraction of the error removed this pass.
#[inline]
pub fn satisfy_distance(a: &mut Vec3, b: &mut Vec3, rest: f32, wa: f32, wb: f32, stiffness: f32) {
    let w = wa + wb;
    if w <= 0.0 {
        return;
    }
    let d = *b - *a;
    let len = d.length();
    if len <= 1e-6 {
        return;
    }
    let corr = d * ((len - rest) / (len * w) * stiffness);
    *a += corr * wa;
    *b -= corr * wb;
}

/// Resolves a point that moved `old → cur` against solid cells, per axis in X, Z, Y order
/// so landing is decided last. Endpoint tests are not enough: a fast point skips a
/// one-cell floor. A point that STARTS inside a solid cell is healed out of the nearest
/// open face instead of moving with collision off, which would let it fall through the
/// world. `old` must be in the same frame `solid` names cells in.
pub fn resolve_through_cells(old: Vec3, cur: Vec3, solid: &impl Fn(IVec3) -> bool) -> Vec3 {
    if solid(voxel_at(old)) {
        return escape_solid(old, solid).unwrap_or(old);
    }
    let mut w = old;
    w.x = sweep_axis(w, 0, cur.x, solid);
    w.z = sweep_axis(w, 2, cur.z, solid);
    w.y = sweep_axis(w, 1, cur.y, solid);
    w
}

/// Moves `w`'s `axis` coordinate toward `target` one cell boundary at a time and parks it
/// just outside the first solid cell it hits. Returns the resolved coordinate.
pub fn sweep_axis(w: Vec3, axis: usize, target: f32, solid: &impl Fn(IVec3) -> bool) -> f32 {
    let start = w[axis];
    if !(start.is_finite() && target.is_finite()) {
        return target;
    }
    let target = target.clamp(start - MAX_SWEEP, start + MAX_SWEEP);
    let mut probe = w;
    if target > start {
        let mut face = start.floor() + 1.0;
        while face <= target {
            probe[axis] = face + FACE_EPS;
            if solid(voxel_at(probe)) {
                return face - FACE_EPS;
            }
            face += 1.0;
        }
    } else {
        let mut face = start.floor();
        while target < face {
            probe[axis] = face - FACE_EPS;
            if solid(voxel_at(probe)) {
                return face + FACE_EPS;
            }
            face -= 1.0;
        }
    }
    target
}

/// The nearest point just outside `w`'s cell through a face whose neighbour is open.
pub fn escape_solid(w: Vec3, solid: &impl Fn(IVec3) -> bool) -> Option<Vec3> {
    let cell = voxel_at(w);
    let lo = cell.as_vec3();
    let mut best: Option<(f32, Vec3)> = None;
    for axis in 0..3 {
        let exits = [
            (w[axis] - lo[axis], lo[axis] - FACE_EPS, -1),
            (lo[axis] + 1.0 - w[axis], lo[axis] + 1.0 + FACE_EPS, 1),
        ];
        for (dist, coord, step) in exits {
            let mut neighbour = cell;
            neighbour[axis] += step;
            if solid(neighbour) || best.is_some_and(|(d, _)| d <= dist) {
                continue;
            }
            let mut out = w;
            out[axis] = coord;
            best = Some((dist, out));
        }
    }
    best.map(|(_, out)| out)
}

/// Pushes `p` out of the vertical capsule (segment `bottom..top`, `radius`) radially
/// from its axis; `None` when outside. A body's capsule is what a cloth should flow
/// around: pushing straight out sideways keeps a flag from riding up over a head.
pub fn push_out_of_capsule(p: Vec3, bottom: Vec3, top: Vec3, radius: f32) -> Option<Vec3> {
    let axis = top - bottom;
    let len2 = axis.length_squared();
    let t = if len2 > 0.0 {
        ((p - bottom).dot(axis) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = bottom + axis * t;
    let d = p - closest;
    let dist2 = d.length_squared();
    if dist2 >= radius * radius {
        return None;
    }
    let dist = dist2.sqrt();
    let dir = if dist > 1e-5 { d / dist } else { Vec3::X };
    Some(closest + dir * radius)
}

#[cfg(test)]
mod tests;
