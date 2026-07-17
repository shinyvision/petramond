//! Shared world-space mob body geometry.
//!
//! Long bodies are represented by the same overlapping, axis-aligned segments
//! for terrain movement, pairwise solid motion, soft contact, placement,
//! client picking, and server-side target validation.

use crate::mathh::Vec3;

use super::MobSize;

mod motion;
mod queries;
#[cfg(test)]
mod tests;

pub(crate) use motion::{
    resolve_body_motion, terrain_safe_motion_prefix, BodyMotion, SolidMotionSolver,
};
pub(crate) use queries::{
    append_body_supports, body_has_peer_support, body_overlaps_block_boxes, body_pose_fits,
    body_separation, body_separation_from_body, clamp_body_yaw, closest_body_ray_hit,
};

type WorldBox = ([f32; 3], [f32; 3]);

fn segment_offsets(size: MobSize) -> impl Iterator<Item = f32> {
    let hw = size.half_width;
    let hl = size.half_length.unwrap_or(hw);
    let segments = size.body_segments();
    let reach = hl - hw;

    (0..segments).map(move |i| {
        if segments == 1 {
            0.0
        } else {
            -reach + 2.0 * reach * (i as f32) / (segments - 1) as f32
        }
    })
}

fn segment_centre(pos: Vec3, yaw: f32, offset: f32) -> Vec3 {
    pos + Vec3::new(-yaw.sin(), 0.0, -yaw.cos()) * offset
}

fn segment_centres(pos: Vec3, yaw: f32, size: MobSize) -> impl Iterator<Item = Vec3> {
    segment_offsets(size).map(move |offset| segment_centre(pos, yaw, offset))
}

/// World-space boxes covering a mob body at `pos` and `yaw`.
///
/// Ordinary bodies yield one square box. A body with `half_length` yields a
/// run of overlapping square boxes along its facing axis, matching the solid
/// collision staircase at diagonal yaws without filling the enclosing
/// square's empty corners.
pub(crate) fn body_boxes(pos: Vec3, yaw: f32, size: MobSize) -> impl Iterator<Item = (Vec3, Vec3)> {
    let hw = size.half_width;
    segment_centres(pos, yaw, size).map(move |centre| {
        (
            Vec3::new(centre.x - hw, centre.y, centre.z - hw),
            Vec3::new(centre.x + hw, centre.y + size.height, centre.z + hw),
        )
    })
}

fn arc_component_bounds(
    from: f32,
    to: f32,
    critical_base: f32,
    value: impl Fn(f32) -> f32,
) -> (f32, f32) {
    let lo = from.min(to);
    let hi = from.max(to);
    let a = value(from);
    let b = value(to);
    let mut min = a.min(b);
    let mut max = a.max(b);
    let first = ((lo - critical_base) / std::f32::consts::PI).ceil() as i32;
    let last = ((hi - critical_base) / std::f32::consts::PI).floor() as i32;
    for k in first..=last {
        let at = critical_base + k as f32 * std::f32::consts::PI;
        let v = value(at);
        min = min.min(v);
        max = max.max(v);
    }
    (min, max)
}

fn wrap_angle(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(std::f32::consts::TAU);
    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}
