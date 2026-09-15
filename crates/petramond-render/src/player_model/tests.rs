use super::*;
use petramond::player::model::player_model;
use petramond::player::rigs::{self, Presenter};
use petramond_math::world_pos::WorldPos;

/// The body rig's main arm is the authored LEFT one, which the yaw+π
/// placement shows on the visual right; the off arm is the authored right.
const HELD_SHOULDER_BONE: &str = "left_shoulder";
const HELD_ELBOW_BONE: &str = "left_elbow";
const OFF_ELBOW_BONE: &str = "right_elbow";

fn body_rig() -> &'static Rig {
    rigs::presented(Presenter::Body).expect("the body rig").1
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
    let mut inst = instance();
    inst.head_yaw = 0.35;
    inst.head_pitch = -0.18;
    inst.locomotion.swim.weight = 1.0;
    model.apply_head_look(&mut pose, head, inst.head_yaw, inst.head_pitch);
    let pivot = pose[head].transform_point3(model.bones[head].pivot);
    locomotion::stabilize_swim_gaze(model, &mut pose, head, &inst);
    let target = Quat::from_rotation_y(inst.head_yaw)
        * Quat::from_rotation_x(inst.head_pitch)
        * petramond_world::bbmodel::euler_quat(model.bones[head].rotation);
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

fn instance() -> PlayerRenderInstance {
    PlayerRenderInstance {
        emitter_tint: [1.0; 3],
        emitter_self_lit: 0.0,
        pos: WorldPos::new(4.0, 70.0, -3.0),
        body_yaw: 0.0,
        head_yaw: 0.0,
        head_pitch: 0.0,
        anim_time: 0.0,
        walk_weight: 0.0,
        sneak_weight: 0.0,
        locomotion: Default::default(),
        sleeping: false,
        seated: false,
        seat_tilt: petramond_math::math::Tilt::LEVEL,
        hurt: 0.0,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        bones: Default::default(),
    }
}

fn bake(inst: &PlayerRenderInstance) -> Vec<ItemVertex> {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    let (n, _, _) = build_player_body(
        body_rig(),
        LightEnv::IDENTITY,
        inst,
        petramond_math::math::IVec3::ZERO,
        &[],
        None,
        &mut v,
        &mut i,
    );
    assert_eq!(n as usize, i.len());
    v
}

#[test]
fn self_lit_players_keep_fire_and_hurt_tints_in_darkness() {
    let mut inst = instance();
    inst.emitter_tint = [1.0, 0.7, 0.4];
    inst.hurt = 0.5;
    let daylight = bake(&inst);
    inst.skylight = 0;
    let dark = bake(&inst);
    inst.emitter_self_lit = 1.0;
    let (mut lit, mut indices) = (Vec::new(), Vec::new());
    build_player_body(
        body_rig(),
        LightEnv {
            sky_scale: 0.0,
            sky_color: [0.6, 0.7, 1.0],
        },
        &inst,
        petramond_math::math::IVec3::ZERO,
        &[],
        None,
        &mut lit,
        &mut indices,
    );
    assert!(!lit.is_empty());
    for ((day, lit), dark) in daylight.iter().zip(&lit).zip(&dark) {
        assert_eq!(
            lit.tint, day.tint,
            "body light preserves fire tint and hurt flash"
        );
        assert_eq!(lit.shade, day.shade);
        assert!(
            dark.tint[0] < lit.tint[0],
            "ordinary bodies still follow world light"
        );
    }
}

/// The two hand attach frames of a body at rest.
fn hands(inst: &PlayerRenderInstance) -> (Mat4, Mat4) {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    let (_, hand, off) = build_player_body(
        body_rig(),
        LightEnv::IDENTITY,
        inst,
        petramond_math::math::IVec3::ZERO,
        &[],
        None,
        &mut v,
        &mut i,
    );
    (hand, off)
}

fn hand(inst: &PlayerRenderInstance) -> Mat4 {
    hands(inst).0
}

fn off_hand(inst: &PlayerRenderInstance) -> Mat4 {
    hands(inst).1
}

#[test]
fn body_bakes_and_walks_and_looks() {
    // Rest pose bakes geometry standing at the feet.
    let rest = bake(&instance());
    assert!(!rest.is_empty(), "player model bakes geometry");

    // Walking at two phases differs (limbs swing).
    let mut walking = instance();
    walking.walk_weight = 1.0;
    walking.anim_time = 0.0;
    let a = bake(&walking);
    walking.anim_time = 0.25;
    let b = bake(&walking);
    assert!(
        a.iter().zip(&b).any(|(x, y)| x.pos != y.pos),
        "walk animation moves the limbs"
    );

    // Head-look moves geometry while idle (the head bone override is wired).
    let mut turned = instance();
    turned.head_yaw = 0.6;
    turned.head_pitch = 0.3;
    let looked = bake(&turned);
    assert!(
        rest.iter().zip(&looked).any(|(x, y)| x.pos != y.pos),
        "head look poses the head"
    );
}

#[test]
fn sneak_weight_poses_the_crouch_and_replaces_the_walk_cycle() {
    // Full sneak while standing still: a crouch stance, not the upright rest.
    let rest = bake(&instance());
    let mut crouched = instance();
    crouched.sneak_weight = 1.0;
    let stance = bake(&crouched);
    assert!(
        rest.iter().zip(&stance).any(|(a, b)| a.pos != b.pos),
        "the sneak stance poses the body"
    );

    // A STILL sneaker holds the clip's first frame: the walk phase must not
    // leak into the stance.
    crouched.anim_time = 0.4;
    let stance_later = bake(&crouched);
    assert!(
        stance
            .iter()
            .zip(&stance_later)
            .all(|(a, b)| a.pos == b.pos),
        "standing sneak freezes on the sneak clip's frame 0"
    );

    // A MOVING sneaker animates through the sneak clip (its own cycle)...
    crouched.walk_weight = 1.0;
    crouched.anim_time = 0.1;
    let step_a = bake(&crouched);
    crouched.anim_time = 0.35;
    let step_b = bake(&crouched);
    assert!(
        step_a.iter().zip(&step_b).any(|(a, b)| a.pos != b.pos),
        "sneak-walking advances the sneak cycle"
    );

    // ...and that cycle is the sneak clip, not the upright walk.
    let mut upright = instance();
    upright.walk_weight = 1.0;
    upright.anim_time = 0.1;
    let walking = bake(&upright);
    assert!(
        walking.iter().zip(&step_a).any(|(a, b)| a.pos != b.pos),
        "sneak-walking is a different cycle than the upright walk"
    );
}

#[test]
fn seated_swings_the_thighs_forward_and_hangs_the_shins() {
    // Seated (mounted): the height shrinks by roughly a thigh (the legs
    // fold), the lowest geometry rises off the anchor (no foot at y=0 —
    // the shins hang from the forward knees), and the knees stick out
    // toward the FACING (+Z at engine yaw 0), while the torso stays
    // upright (still much taller than a lying body).
    let standing = bake(&instance());
    let mut riding = instance();
    riding.seated = true;
    let seated = bake(&riding);
    let span = |v: &[ItemVertex], axis: usize| {
        let lo = v.iter().map(|x| x.pos[axis]).fold(f32::MAX, f32::min);
        let hi = v.iter().map(|x| x.pos[axis]).fold(f32::MIN, f32::max);
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
    // Direction proof, not a reach pin: the folded legs must extend the
    // body's FACING side (+Z at yaw 0 — the head already reaches part of
    // the way there, so the margin is what the knees add past it).
    let (_, stand_z_hi) = span(&standing, 2);
    let (_, sit_z_hi) = span(&seated, 2);
    assert!(
        sit_z_hi > stand_z_hi + 0.05,
        "the knees extend toward the facing: {sit_z_hi} vs {stand_z_hi}"
    );
}

#[test]
fn sleeping_lies_the_body_flat() {
    // Standing spans ~1.85 blocks of height; asleep the same model must lie
    // flat (height collapses to body thickness) and stretch horizontally.
    let standing = bake(&instance());
    let mut asleep = instance();
    asleep.sleeping = true;
    let lying = bake(&asleep);
    let height = |v: &[ItemVertex]| {
        let ys: Vec<f32> = v.iter().map(|x| x.pos[1]).collect();
        ys.iter().fold(f32::MIN, |a, &b| a.max(b)) - ys.iter().fold(f32::MAX, |a, &b| a.min(b))
    };
    assert!(height(&standing) > 1.5, "standing body is tall");
    assert!(
        height(&lying) < 0.8,
        "sleeping body lies flat: {}",
        height(&lying)
    );
    // The body rests on the mattress plane: the torso (2 px half-thickness)
    // sits on it, and only the deeper head cube (4 px + hat inflate) may
    // nestle slightly below — into the pillow — never the whole body.
    let min_y = lying.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
    assert!(
        min_y >= (asleep.pos.y - 0.2) as f32,
        "only a pillow-deep nestle below the mattress: {min_y}"
    );
}

#[test]
fn walk_weight_blends_between_rest_and_the_full_cycle() {
    // A half-weight walk pose sits strictly between rest and the full cycle:
    // it differs from both, so stopping eases through intermediate poses
    // instead of flipping rest↔walk in one frame.
    let mut inst = instance();
    inst.anim_time = 0.25;
    inst.walk_weight = 0.0;
    let rest = bake(&inst);
    inst.walk_weight = 1.0;
    let full = bake(&inst);
    inst.walk_weight = 0.5;
    let half = bake(&inst);
    assert!(
        rest.iter().zip(&half).any(|(a, b)| a.pos != b.pos),
        "half blend differs from rest"
    );
    assert!(
        full.iter().zip(&half).any(|(a, b)| a.pos != b.pos),
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

    // The fist's pose in the TORSO's frame — what "the arm moved" means.
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
            // Non-vacuous: the OTHER arm swings at these very phases.
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

/// The third-person bbmodel attach adds `Rx(-90°)` and NOTHING ELSE.
///
/// It carried an extra `Ry(180°)` until 2026-08-22, which turned every
/// bbmodel item end-over-end in the fist relative to its own Blockbench
/// preview — the game contradicting the model file. It hid for as long as
/// it did because the only bbmodel items were the buckets, which are
/// four-fold symmetric about exactly the axis it flipped; it surfaced the
/// moment an item with a top and a bottom went in a hand.
///
/// Pinned as DIRECTIONS rather than a matrix so it reads as the contract
/// it is: display "up" (+Y) points forward out of the fist, display
/// "forward" (+Z) points up, and neither the item's left nor its top is
/// mirrored. The bucket is the subject because its authored third-person
/// rotation is identity, so what is left IS the attach.
#[test]
fn the_third_person_attach_reorients_without_flipping_the_item() {
    use petramond_world::item::{ItemRenderKind, ItemType};

    let bucket = ItemType::by_name("petramond:wooden_bucket").expect("engine item");
    let ItemRenderKind::Model(kind) = bucket.render_kind() else {
        panic!("the bucket is a bbmodel item")
    };
    // In the ARM's own frame (the rest arm hangs unrotated, so its axes are
    // the authored model's: +Y up, −Z the body's front).
    let m = held_model_at(Grip::body(Mat4::IDENTITY), kind);
    let dir = |v: Vec3| m.transform_vector3(v).normalize();

    let forward = dir(Vec3::Y);
    assert!(
        forward.z < -0.9,
        "display up must point out of the fist along the body's front, got {forward:?}"
    );
    let up = dir(Vec3::Z);
    assert!(
        up.y > 0.9,
        "display forward must point UP, not down — a down y is the old spurious yaw: {up:?}"
    );
    let right = dir(Vec3::X);
    assert!(
        right.x > 0.9,
        "the item's own +X must not be mirrored in the fist, got {right:?}"
    );
}

/// The load-bearing identity behind every off-hand pose in the engine:
/// CONJUGATING a display transform by the x-flip IS
/// `DisplayTransform::left_hand`.
///
/// It is why neither view needs a per-hand rule for a claimed pose — the
/// off-hand paths already conjugate the frame the pose rides in. Off-hand
/// mirroring has been got wrong here before, and the failure is always
/// silent: the item hangs somewhere plausible and wrong. Pin the algebra so
/// a change of euler convention argues with a test, not with a playtest.
#[test]
fn conjugating_a_display_transform_is_exactly_the_left_hand_rule() {
    use petramond_world::block_model::DisplayTransform;

    for pose in [
        DisplayTransform {
            rotation: [0.0, 0.0, 0.0],
            translation: [1.5, -3.0, -5.0],
            ..Default::default()
        },
        DisplayTransform {
            rotation: [-16.0, 40.0, -25.0],
            translation: [0.0, 7.0, -2.0],
            ..Default::default()
        },
    ] {
        let conjugated = mirror_local(pose.base_matrix());
        let authored = pose.left_hand().base_matrix();
        let (a, b) = (conjugated.to_cols_array(), authored.to_cols_array());
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!(
                (x - y).abs() < 1e-5,
                "element {i} of {pose:?}: conjugated {x} vs left_hand {y}"
            );
        }
    }
}

/// A pose that changes nothing must leave the attach frame BIT-identical,
/// not merely close: every hand without a mod pose takes this path every
/// frame, and a matrix round trip there would move every held item in the
/// game by a rounding error.
#[test]
fn an_identity_pose_leaves_the_hand_frame_untouched() {
    let inst = instance();
    let frame = hand(&inst);
    assert_eq!(posed_hand(frame, &Default::default(), false), frame);
    assert_eq!(posed_hand(frame, &Default::default(), true), frame);
}

#[test]
fn held_grip_is_on_the_visual_right_side() {
    let inst = instance();
    let grip = hand(&inst).transform_point3(HAND_GRIP_PX);
    assert!(
        grip.x < inst.pos.x as f32,
        "yaw 0 player-right is camera-right/world -X, grip at {grip:?}"
    );
}

#[test]
fn off_hand_grip_is_on_the_visual_left_side() {
    let inst = instance();
    let grip_local = Vec3::new(-HAND_GRIP_PX.x, HAND_GRIP_PX.y, HAND_GRIP_PX.z);
    let grip = off_hand(&inst).transform_point3(grip_local);
    assert!(
        grip.x > inst.pos.x as f32,
        "yaw 0 player-left is world +X, off grip at {grip:?}"
    );
}
