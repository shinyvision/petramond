//! Posing one third-person player body: the rigs catalog's body rig turned
//! into final model-space bone matrices plus the placement that stands them
//! about the body's feet — the pose array the renderer skins, and nothing it
//! has to compute.
//!
//! Composition, in order: the locomotion table's clips blended by their
//! weights (`locomotion.rs`) as the GROUND, the body animator's graph
//! evaluated over it (actions override and add to the walk), the sleep and
//! seat poses, the head-look override on the `head` bone (compensated for
//! the yaw the animator added to the row's `twist` bones, so the gaze stays
//! put), then the claimed bone offsets.
//!
//! The model is authored front = −Z (the skin's face texture sits on the north
//! face), while engine yaw 0 faces +Z, so the body is placed with `yaw + π`.

use glam::{Mat4, Quat, Vec3};
use petramond::player::model::{PLAYER_HIP_HEIGHT, PLAYER_MODEL_SCALE};
use petramond::player::rigs::Rig;
use petramond_anim::LocalPose;
use petramond_render::HeldItemFrame;

use super::body::BodyAnimator;
use super::claims::AnimatorInputs;
use super::locomotion;
use super::motion::{BodyState, BoneOffset};

const LIE_LIFT: f32 = 2.2 * PLAYER_MODEL_SCALE;

const SEATED_HIP_BEND: f32 = 1.35;

pub(super) struct BodyDrive<'a> {
    pub animator: &'a mut BodyAnimator,
    pub frames: Option<&'a [HeldItemFrame; 2]>,
    pub inputs: AnimatorInputs<'a>,
    pub dt: f32,
}

/// Pose one player body of `rig`, appending its final model-space bone
/// matrices (rig pixels) to `out` and answering the body's PLACEMENT: the
/// transform that stands the posed rig about its feet (yaw, seat lean or the
/// lying turn, model scale) — the renderer prepends only the translation to
/// the feet. With a `drive` the locomotion pose is the animator's ground and
/// the animator's pose is what draws; without one (no body graph) the
/// locomotion pose draws as it is.
pub(super) fn pose_body(
    rig: &Rig,
    state: &BodyState,
    bones: &[BoneOffset],
    drive: Option<&mut BodyDrive<'_>>,
    out: &mut Vec<Mat4>,
) -> Mat4 {
    let model = &rig.model;
    let layers = locomotion::layers(model, state);
    let mut twist_total = 0.0;
    let mut pose = match drive {
        Some(drive) => {
            let mut ground = LocalPose::rest(model.bones().len());
            for (anim, time, weight) in &layers {
                ground.add_clip(anim, *time, weight.clamp(0.0, 1.0));
            }
            drive
                .animator
                .update(state, drive.frames, drive.inputs, drive.dt, &ground);
            let local = drive.animator.pose();
            for &bone in &rig.twist {
                twist_total += (local.rotation(bone).y - ground.rotation(bone).y).to_radians();
            }
            local.resolve(model)
        }
        None if layers.is_empty() => model.rest_pose(),
        None => model.pose_layers(&layers),
    };
    let head_animated = |hb: usize| layers.iter().any(|(a, _, _)| a.affects_bone(hb));

    if state.sleeping {
        out.extend_from_slice(&pose);
        return Mat4::from_translation(Vec3::new(0.0, LIE_LIFT, 0.0))
            * Mat4::from_rotation_y(state.body_yaw)
            * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
    }

    if state.seated {
        for (hip, knee) in [("leftLeg", "left_knee"), ("rightLeg", "right_knee")] {
            if let Some(bone) = model.bone_named(hip) {
                model.apply_bone_rotation(&mut pose, bone, Quat::from_rotation_x(SEATED_HIP_BEND));
            }
            if let Some(bone) = model.bone_named(knee) {
                model.apply_bone_rotation(&mut pose, bone, Quat::from_rotation_x(-SEATED_HIP_BEND));
            }
        }
    }

    // The GAZE layers stay procedural on top of the data, deliberately: a
    // keyframe file cannot know where this viewer's player is looking.
    // Head-look compensates the twist the data put on the torso (the engine
    // is the sampler, so it knows), and the aim term leans each swinging
    // shoulder into the look pitch so a punch goes where the eyes do.
    if let Some(hb) = model.head_bone() {
        if !head_animated(hb) {
            model.apply_head_look(
                &mut pose,
                hb,
                state.head_yaw - twist_total,
                state.head_pitch,
            );
            locomotion::stabilize_swim_gaze(model, &mut pose, hb, state);
        }
    }
    // Claimed bone offsets LAST, so they compose on top of every engine layer
    // (walk, sneak, head-look, the animator's actions) rather than
    // fighting one. `apply_bone_offset` carries each through the bone's
    // descendants, so one shoulder offset raises the whole arm AND the item in
    // its fist — the held-pose seam never has to know.
    //
    // The engine's own layers are NOT claims, unlike the speed scale and the
    // barred actions, and the asymmetry is deliberate: a claim is replicated
    // authority, while an animation is derived presentation every viewer
    // computes for itself from a few replicated flags. Folding the walk cycle
    // into claims would put a sampled pose per bone per body on the wire to
    // buy nothing. That is exactly why `BonePoseMode::Replace` exists — a claim
    // needs a way to overrule a layer it cannot take part in.
    for offset in bones {
        let translation = Vec3::from(offset.translation) / 16.0 / PLAYER_MODEL_SCALE;
        if offset.hold {
            model.hold_bone(
                &mut pose,
                offset.bone,
                Vec3::from(offset.rotation),
                translation,
            );
        } else {
            model.apply_bone_offset(
                &mut pose,
                offset.bone,
                petramond_world::bbmodel::display_euler_quat(Vec3::from(offset.rotation)),
                translation,
            );
        }
    }
    out.extend_from_slice(&pose);

    // Authored front is −Z; engine yaw 0 faces +Z — hence the π. A seated
    // body leans with its mount: the whole rig tilts about the HIP pivot
    // (the point the seat carries — see `mob::riding::seat_world_pos`), so
    // the legs keep their authored place in the cart's own frame on a slope
    // instead of an upright body's knees driving through the tilted floor.
    let lean = if state.seated && !state.seat_tilt.is_level() {
        let hip = Vec3::new(0.0, PLAYER_HIP_HEIGHT, 0.0);
        Mat4::from_translation(hip) * state.seat_tilt.rotation() * Mat4::from_translation(-hip)
    } else {
        Mat4::IDENTITY
    };
    Mat4::from_rotation_y(state.body_yaw + std::f32::consts::PI)
        * lean
        * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE))
}

#[cfg(test)]
mod tests;
