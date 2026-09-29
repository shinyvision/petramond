//! Plane geometry for bodies that face by yaw. Mob convention: yaw 0 faces −Z and the body's
//! forward is `(−sin yaw, −cos yaw)` ([`mob_facing_xz`]).

use std::f32::consts::{PI, TAU};

use mod_sdk::mob_facing_xz;

pub fn yaw_toward(from: [f64; 3], to: [f64; 3]) -> Option<f32> {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (dx * dx + dz * dz > 1.0e-8).then(|| (-dx).atan2(-dz) as f32)
}

pub fn wrap(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

pub fn turn_toward(from: f32, to: f32, max_step: f32) -> f32 {
    wrap(from + wrap(to - from).clamp(-max_step, max_step))
}

/// Cosine of the horizontal angle between the body's forward and the way to `to`; `None` when
/// `to` stands on the body's own axis and has no bearing.
pub fn bearing_cos(yaw: f32, from: [f64; 3], to: [f64; 3]) -> Option<f32> {
    let (dx, dz) = ((to[0] - from[0]) as f32, (to[2] - from[2]) as f32);
    let len = dx.hypot(dz);
    if len < 1.0e-4 {
        return None;
    }
    let [fx, fz] = mob_facing_xz(yaw);
    Some((fx * dx + fz * dz) / len)
}

/// Whether `to` lies within `half_arc_deg` either side of the body's forward. A point with no
/// bearing is in no arc.
pub fn within_arc(yaw: f32, from: [f64; 3], to: [f64; 3], half_arc_deg: f32) -> bool {
    bearing_cos(yaw, from, to).is_some_and(|c| c >= half_arc_deg.to_radians().cos())
}

/// An upright body: feet centre, half width, height.
#[derive(Clone, Copy, Debug)]
pub struct Upright {
    pub feet: [f64; 3],
    pub half_width: f32,
    pub height: f32,
}

impl Upright {
    pub fn at(&self, fraction: f32) -> [f64; 3] {
        let [x, y, z] = self.feet;
        [x, y + f64::from(self.height * fraction), z]
    }
}

/// Edge-to-edge distance between two upright bodies.
pub fn gap(a: &Upright, b: &Upright) -> f32 {
    let (dx, dz) = (b.feet[0] - a.feet[0], b.feet[2] - a.feet[2]);
    let across = ((dx * dx + dz * dz).sqrt() as f32 - a.half_width - b.half_width).max(0.0);
    let (a0, a1) = (a.feet[1], a.feet[1] + f64::from(a.height));
    let (b0, b1) = (b.feet[1], b.feet[1] + f64::from(b.height));
    let up = (b0 - a1).max(a0 - b1).max(0.0) as f32;
    across.hypot(up)
}

/// Whether the segment `from → to` passes through the box `min..max`.
pub fn segment_meets_box(from: [f64; 3], to: [f64; 3], min: [f64; 3], max: [f64; 3]) -> bool {
    let (mut near, mut far) = (0.0f64, 1.0f64);
    for axis in 0..3 {
        let d = to[axis] - from[axis];
        if d.abs() < 1.0e-12 {
            if from[axis] < min[axis] || from[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let a = (min[axis] - from[axis]) / d;
        let b = (max[axis] - from[axis]) / d;
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return false;
        }
    }
    true
}

pub fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}
