use glam::{Mat4, Quat, Vec3};
use petramond::player::model::player_model;
use petramond::player::rigs::{self, Presenter, Rig};
use petramond_math::face::Face;
use petramond_world::bbmodel::{euler_quat, face_corners};

use super::{locomotion, pose_body};
use crate::animation::BodyState;

const HELD_SHOULDER_BONE: &str = "left_shoulder";
const HELD_ELBOW_BONE: &str = "left_elbow";
const OFF_ELBOW_BONE: &str = "right_elbow";

fn body_rig() -> &'static Rig {
    rigs::presented(Presenter::Body).expect("the body rig").1
}

fn bake(state: &BodyState) -> Vec<Vec3> {
    let model = &body_rig().model;
    let mut pose = Vec::new();
    let placement = pose_body(body_rig(), state, &[], None, &mut pose);
    assert_eq!(pose.len(), model.bones().len(), "one matrix per bone");
    model
        .cubes
        .iter()
        .flat_map(|cube| {
            let s_cube = Mat4::from_translation(cube.origin)
                * Mat4::from_quat(euler_quat(cube.rotation))
                * Mat4::from_translation(-cube.origin);
            let m = placement * pose[cube.bone] * s_cube;
            Face::ALL
                .into_iter()
                .flat_map(move |face| face_corners(face, cube.from, cube.to))
                .map(move |c| m.transform_point3(Vec3::from(c)))
        })
        .collect()
}

fn differs(a: &[Vec3], b: &[Vec3]) -> bool {
    a.iter().zip(b).any(|(x, y)| x != y)
}

#[test]
fn swimming_gaze_stays_on_target_through_torso_rotation() {
    let model = player_model();
    let head = model.head_bone().unwrap();
    let body = model.bone_named("body").unwrap();
    let mut pose = model.rest_pose();
    model.apply_bone_rotation(
        &mut pose,
        body,
        Quat::from_euler(glam::EulerRot::XYZ, -0.8, 0.25, 0.12),
    );
    let mut state = BodyState {
        head_yaw: 0.35,
        head_pitch: -0.18,
        ..Default::default()
    };
    state.locomotion.swim.weight = 1.0;
    model.apply_head_look(&mut pose, head, state.head_yaw, state.head_pitch);
    let pivot = pose[head].transform_point3(model.bones[head].pivot);
    locomotion::stabilize_swim_gaze(model, &mut pose, head, &state);
    let target = Quat::from_rotation_y(state.head_yaw)
        * Quat::from_rotation_x(state.head_pitch)
        * euler_quat(model.bones[head].rotation);
    assert!(
        pose[head]
            .to_scale_rotation_translation()
            .1
            .dot(target)
            .abs()
            > 1.0 - 1e-5
    );
    assert!(
        pose[head]
            .transform_point3(model.bones[head].pivot)
            .distance(pivot)
            < 1e-4
    );
}

#[test]
fn body_poses_and_walks_and_looks() {
    let rest = bake(&BodyState::default());
    assert!(!rest.is_empty(), "player model bakes geometry");

    let mut walking = BodyState {
        walk_weight: 1.0,
        ..Default::default()
    };
    let a = bake(&walking);
    walking.anim_time = 0.25;
    let b = bake(&walking);
    assert!(differs(&a, &b), "walk animation moves the limbs");

    let turned = BodyState {
        head_yaw: 0.6,
        head_pitch: 0.3,
        ..Default::default()
    };
    assert!(differs(&rest, &bake(&turned)), "head look poses the head");
}

#[test]
fn sneak_weight_poses_the_crouch_and_replaces_the_walk_cycle() {
    let rest = bake(&BodyState::default());
    let mut crouched = BodyState {
        sneak_weight: 1.0,
        ..Default::default()
    };
    let stance = bake(&crouched);
    assert!(differs(&rest, &stance), "the sneak stance poses the body");

    crouched.anim_time = 0.4;
    assert_eq!(
        stance,
        bake(&crouched),
        "standing sneak freezes on the sneak clip's frame 0"
    );

    crouched.walk_weight = 1.0;
    crouched.anim_time = 0.1;
    let step_a = bake(&crouched);
    crouched.anim_time = 0.35;
    let step_b = bake(&crouched);
    assert!(
        differs(&step_a, &step_b),
        "sneak-walking advances the sneak cycle"
    );

    let upright = BodyState {
        walk_weight: 1.0,
        anim_time: 0.1,
        ..Default::default()
    };
    assert!(
        differs(&bake(&upright), &step_a),
        "sneak-walking is a different cycle than the upright walk"
    );
}

#[test]
fn seated_swings_the_thighs_forward_and_hangs_the_shins() {
    let standing = bake(&BodyState::default());
    let seated = bake(&BodyState {
        seated: true,
        ..Default::default()
    });
    let span = |v: &[Vec3], axis: usize| {
        let lo = v.iter().map(|x| x[axis]).fold(f32::MAX, f32::min);
        let hi = v.iter().map(|x| x[axis]).fold(f32::MIN, f32::max);
        (lo, hi)
    };
    let (stand_lo, stand_hi) = span(&standing, 1);
    let (sit_lo, sit_hi) = span(&seated, 1);
    assert!(
        (stand_hi - stand_lo) - (sit_hi - sit_lo) > 0.25,
        "sitting folds the legs: {} vs {}",
        stand_hi - stand_lo,
        sit_hi - sit_lo
    );
    assert!(
        sit_lo > stand_lo + 0.2,
        "the shins hang above the anchor: {sit_lo} vs {stand_lo}"
    );
    assert!(
        sit_hi - sit_lo > 1.0,
        "the torso stays upright (not lying): {}",
        sit_hi - sit_lo
    );
    let (_, stand_z_hi) = span(&standing, 2);
    let (_, sit_z_hi) = span(&seated, 2);
    assert!(
        sit_z_hi > stand_z_hi + 0.05,
        "the knees extend toward the facing: {sit_z_hi} vs {stand_z_hi}"
    );
}

#[test]
fn sleeping_lies_the_body_flat() {
    let standing = bake(&BodyState::default());
    let lying = bake(&BodyState {
        sleeping: true,
        ..Default::default()
    });
    let height = |v: &[Vec3]| {
        let lo = v.iter().map(|x| x.y).fold(f32::MAX, f32::min);
        let hi = v.iter().map(|x| x.y).fold(f32::MIN, f32::max);
        hi - lo
    };
    assert!(height(&standing) > 1.5, "standing body is tall");
    assert!(
        height(&lying) < 0.8,
        "sleeping body lies flat: {}",
        height(&lying)
    );
    let min_y = lying.iter().map(|v| v.y).fold(f32::MAX, f32::min);
    assert!(
        min_y >= -0.2,
        "only a pillow-deep nestle below the mattress: {min_y}"
    );
}

#[test]
fn walk_weight_blends_between_rest_and_the_full_cycle() {
    let at = |walk_weight| {
        bake(&BodyState {
            anim_time: 0.25,
            walk_weight,
            ..Default::default()
        })
    };
    let (rest, half, full) = (at(0.0), at(0.5), at(1.0));
    assert!(differs(&rest, &half), "half blend differs from rest");
    assert!(
        differs(&full, &half),
        "half blend differs from the full cycle"
    );
}

/// A HELD arm ignores the walk cycle; a COMPOSED one rides it.
///
/// This is the whole difference between a nudge and a STANCE, and the
/// failure is quiet: a guard raised with a composed offset still swings
/// with the stride, because the swing is underneath it. `walk` and `sneak`
/// both drive the shoulder AND the elbow, so this cannot be checked
/// against a standing body — the animation has to be running.
///
/// Measured RELATIVE TO THE TORSO, because the gaits also bob the root and
/// a stance is not supposed to stop the body moving — only the arm.
///
/// It also pins the part that surprises: holding a SHOULDER does not
/// freeze the arm. Descendants keep their own animation relative to the
/// held bone (which is what makes a held shoulder usable as a nudge
/// point), so a stance has to hold every joint it owns.
#[test]
fn a_held_arm_ignores_the_walk_cycle_and_a_composed_one_rides_it() {
    let model = player_model();
    let body = model.bone_named("body").expect("torso");
    let shoulder = model.bone_named(HELD_SHOULDER_BONE).expect("main arm");
    let elbow = model.bone_named(HELD_ELBOW_BONE).expect("main forearm");
    let off_elbow = model.bone_named(OFF_ELBOW_BONE).expect("off forearm");
    let rot = Vec3::new(59.0, 19.0, -20.0);

    let fist = |gait: &str, phase: f32, hold_shoulder: bool, hold_elbow: bool, bone: usize| {
        let anim = model.animation(gait).expect(gait);
        let mut pose = model.pose_layers(&[(anim, phase, 1.0)]);
        for (b, hold) in [(shoulder, hold_shoulder), (elbow, hold_elbow)] {
            if hold {
                model.hold_bone(&mut pose, b, rot, Vec3::ZERO);
            } else {
                model.apply_bone_offset(
                    &mut pose,
                    b,
                    petramond_world::bbmodel::display_euler_quat(rot),
                    Vec3::ZERO,
                );
            }
        }
        pose[body].inverse() * pose[bone]
    };
    let moved = |a: Mat4, b: Mat4| {
        a.to_cols_array()
            .iter()
            .zip(b.to_cols_array())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0f32, f32::max)
    };

    for gait in [
        petramond_world::bbmodel::clips::WALK,
        petramond_world::bbmodel::clips::SNEAK,
    ] {
        let rest = fist(gait, 0.0, true, true, elbow);
        for phase in [0.2, 0.45, 0.7] {
            assert!(
                moved(rest, fist(gait, phase, true, true, elbow)) < 1e-4,
                "a held arm must not move through the {gait} cycle"
            );
            assert!(
                moved(
                    fist(gait, 0.0, true, true, off_elbow),
                    fist(gait, phase, true, true, off_elbow)
                ) > 1e-3,
                "the unheld arm must still swing, or this proves nothing"
            );
            assert!(
                moved(rest, fist(gait, phase, false, false, elbow)) > 1e-3,
                "a composed offset rides the {gait} instead of replacing it"
            );
            assert!(
                moved(rest, fist(gait, phase, true, false, elbow)) > 1e-3,
                "holding only the shoulder leaves the elbow animating"
            );
        }
    }
}
