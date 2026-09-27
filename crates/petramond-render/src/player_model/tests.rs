use super::*;
use petramond::player::rigs::{self, Presenter};
use petramond_math::world_pos::WorldPos;

fn body_rig() -> &'static Rig {
    rigs::presented(Presenter::Body).expect("the body rig").1
}

fn standing_placement() -> Mat4 {
    Mat4::from_rotation_y(std::f32::consts::PI) * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE))
}

fn instance() -> PlayerRenderInstance {
    PlayerRenderInstance {
        emitter_tint: [1.0; 3],
        emitter_self_lit: 0.0,
        pos: WorldPos::new(4.0, 70.0, -3.0),
        sleeping: false,
        hurt: 0.0,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        pose: crate::ArenaRange {
            start: 0,
            len: body_rig().model.bones().len() as u32,
        },
        placement: standing_placement(),
    }
}

fn pose(inst: &PlayerRenderInstance) -> (SkinBatch, Mat4, Mat4) {
    let mut batch = SkinBatch::default();
    let rest = body_rig().model.rest_pose();
    let (hand, off) = place_player_body(
        body_rig(),
        inst,
        inst.pose.of(&rest),
        petramond_math::math::IVec3::ZERO,
        &mut batch,
    );
    assert_eq!(batch.instances.len(), 1);
    (batch, hand, off)
}

#[test]
fn a_body_skins_exactly_the_pose_it_is_given() {
    let model = &body_rig().model;
    let mut posed = model.rest_pose();
    posed[0] = Mat4::from_rotation_x(0.7) * posed[0];
    let origin = petramond_math::math::IVec3::new(0, 64, 0);
    let inst = instance();
    let mut batch = SkinBatch::default();
    place_player_body(body_rig(), &inst, &posed, origin, &mut batch);
    let global = Mat4::from_translation(inst.pos.relative_to(origin)) * inst.placement;
    for (slot, bone) in posed.iter().enumerate() {
        assert_eq!(
            batch.palette[slot],
            (global * *bone).to_cols_array_2d(),
            "slot {slot}"
        );
    }
}

#[test]
fn a_body_carries_its_hurt_fire_and_light_to_the_instance() {
    let mut inst = instance();
    inst.emitter_tint = [1.0, 0.7, 0.4];
    inst.hurt = 0.5;
    inst.skylight = 0;
    inst.emitter_self_lit = 1.0;
    let (batch, _, _) = pose(&inst);
    let row = batch.instances[0];
    assert_eq!(
        row.tint,
        crate::lighting::mul3(crate::mob_model::hurt_tint(0.5), [1.0, 0.7, 0.4])
    );
    assert_eq!(row.self_lit, 1.0);
    assert_eq!(row.light[0], 0.0);
    assert_eq!(row.hidden, 0);
    assert_eq!(
        batch.palette.len() as u32,
        crate::skinned::bone_slots(&body_rig().model)
    );
}

fn hands(inst: &PlayerRenderInstance) -> (Mat4, Mat4) {
    let (_, hand, off) = pose(inst);
    (hand, off)
}

fn hand(inst: &PlayerRenderInstance) -> Mat4 {
    hands(inst).0
}

fn off_hand(inst: &PlayerRenderInstance) -> Mat4 {
    hands(inst).1
}

/// The third-person bbmodel attach adds `Rx(-90°)` and NOTHING ELSE.
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
/// mirroring follows this algebra across Euler convention changes.
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
