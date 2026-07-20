use glam::{Mat4, Quat, Vec3};

use super::{Bone, Keyframe};

/// Linearly interpolate a sorted channel track (euler degrees or model-unit
/// offsets) at time `t`. Clamps to the endpoints outside the keyed range.
pub(super) fn sample_track(kfs: &[Keyframe], t: f32) -> Vec3 {
    if kfs.is_empty() {
        return Vec3::ZERO;
    }
    if t <= kfs[0].time {
        return kfs[0].v;
    }
    let last = kfs.len() - 1;
    if t >= kfs[last].time {
        return kfs[last].v;
    }
    for w in kfs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t >= a.time && t <= b.time {
            let span = b.time - a.time;
            let f = if span > 1e-6 {
                (t - a.time) / span
            } else {
                0.0
            };
            return a.v + (b.v - a.v) * f;
        }
    }
    kfs[last].v
}

/// Quaternion from euler degrees (XYZ order — exact for single-axis rotations).
/// Shared with [`crate::render::mob_model`] for the static per-cube tilt.
pub(crate) fn euler_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        glam::EulerRot::XYZ,
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

/// A bone's local pose: the animated position offset translates the bone (and
/// its subtree) in the parent's frame, then the rest + animated rotation turns
/// it about its pivot — matching Blockbench's preview of both channels.
pub(super) fn bone_transform(bone: &Bone, anim_rot: Vec3, anim_pos: Vec3) -> Mat4 {
    Mat4::from_translation(bone.pivot + anim_pos)
        * Mat4::from_quat(euler_quat(bone.rotation + anim_rot))
        * Mat4::from_translation(-bone.pivot)
}

pub(super) fn head_look_transform(bone: &Bone, yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(bone.pivot)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_quat(euler_quat(bone.rotation))
        * Mat4::from_translation(-bone.pivot)
}
