use super::*;

#[test]
fn small_look_turns_move_only_the_head() {
    // Idle, head 30° off the body: under the 45° threshold the body stays put.
    let body = follow_body_yaw(0.0, 30f32.to_radians(), false, 0.016);
    assert!(
        body.abs() < 1e-6,
        "body untouched under the threshold: {body}"
    );
}

#[test]
fn past_the_threshold_the_body_is_dragged_along() {
    // Idle, head 80° off: the body is pulled so the head-body offset is
    // exactly the 45° limit.
    let head = 80f32.to_radians();
    let body = follow_body_yaw(0.0, head, false, 0.016);
    assert!(
        (wrap_angle(head - body) - HEAD_YAW_LIMIT).abs() < 1e-5,
        "offset clamps to the limit"
    );
    // Same on the other side.
    let body = follow_body_yaw(0.0, -head, false, 0.016);
    assert!((wrap_angle(-head - body) + HEAD_YAW_LIMIT).abs() < 1e-5);
}

#[test]
fn walking_realigns_the_body_to_the_look() {
    // While moving the body converges to the head across frames, even when
    // the offset never crosses the drag threshold.
    let head = 30f32.to_radians();
    let mut body = 0.0;
    for _ in 0..120 {
        body = follow_body_yaw(body, head, true, 1.0 / 60.0);
    }
    assert!(
        wrap_angle(head - body).abs() < 0.01,
        "body aligned while walking: {body}"
    );
}

#[test]
fn follow_handles_the_yaw_wrap_seam() {
    // Head just past +π, body just under -π: the true offset is tiny, so the
    // body must not spin the long way round.
    let head = std::f32::consts::PI - 0.05;
    let body0 = -std::f32::consts::PI + 0.05;
    let body = follow_body_yaw(body0, head, false, 0.016);
    assert!(
        wrap_angle(head - body).abs() <= HEAD_YAW_LIMIT + 1e-5,
        "wrap seam does not over-rotate"
    );
}

#[test]
fn pose_walk_weight_eases_in_and_back_to_rest() {
    let mut pose = BodyPose::default();
    pose.advance_gait(1.0 / 60.0, 3.0, 0.0, true, false);
    assert!(pose.moving);
    assert!(
        pose.walk_weight > 0.0 && pose.walk_weight < 1.0,
        "the blend eases rather than snapping: {}",
        pose.walk_weight
    );
    let mid_phase = pose.anim_time;
    assert!(mid_phase > 0.0, "the phase advances while moving");
    // Stop: the weight decays smoothly and eventually clamps to rest.
    for _ in 0..120 {
        pose.advance_gait(1.0 / 60.0, 0.0, 0.0, true, false);
    }
    assert_eq!(pose.walk_weight, 0.0, "stopping settles back to rest");
}

#[test]
fn lerp_angle_crosses_the_wrap_seam_the_short_way() {
    use std::f32::consts::PI;
    let mid = lerp_angle(PI - 0.1, -PI + 0.1, 0.5);
    assert!(
        wrap_angle(mid - PI).abs() < 1e-5,
        "midpoint sits on the seam, not the long way round: {mid}"
    );
}

fn motion(velocity: glam::Vec3, grounded: bool) -> MotionFrame {
    MotionFrame {
        position: glam::Vec3::ZERO,
        velocity,
        yaw: 0.0,
        grounded,
        medium: MovementMedium::Land,
        enabled: true,
        sneaking: false,
    }
}

#[test]
fn airborne_motion_retires_the_stride_and_lands_once() {
    use glam::Vec3;
    let mut pose = BodyPose::default();
    for _ in 0..60 {
        pose.advance(1.0 / 60.0, motion(Vec3::new(0.0, 0.0, 4.0), true));
    }
    let phase = pose.anim_time;
    for _ in 0..30 {
        pose.advance(1.0 / 60.0, motion(Vec3::new(0.0, -8.0, 4.0), false));
    }
    assert_eq!(phase, pose.anim_time, "no running on air");
    assert!(pose.walk_weight < 0.01 && pose.locomotion.airborne > 0.9);
    pose.advance(1.0 / 60.0, motion(Vec3::ZERO, true));
    assert!(pose.locomotion.landing > 0.0);
    for _ in 0..60 {
        pose.advance(1.0 / 60.0, motion(Vec3::ZERO, true));
    }
    assert_eq!(pose.locomotion.landing, 0.0);
}

#[test]
fn backward_motion_selects_backpedal_without_resetting_stride() {
    use glam::Vec3;
    let mut pose = BodyPose::default();
    for _ in 0..60 {
        pose.advance(1.0 / 60.0, motion(Vec3::new(0.0, 0.0, -6.0), true));
    }
    assert!(pose.locomotion.backward > 0.9 && pose.locomotion.run < 0.01);
    let phase = pose.anim_time;
    pose.advance(1.0 / 60.0, motion(Vec3::new(0.0, 0.0, 6.0), true));
    assert!((pose.anim_time - phase).rem_euclid(1.0) < 0.1);
    assert!(
        pose.locomotion.backward > 0.1,
        "direction changes crossfade"
    );
}

#[test]
fn suppression_and_teleports_clear_airborne_history() {
    use glam::Vec3;
    let mut pose = BodyPose::default();
    pose.advance(0.016, motion(Vec3::new(0.0, -10.0, 0.0), false));
    pose.advance(
        0.016,
        MotionFrame {
            position: Vec3::splat(100.0),
            ..motion(Vec3::ZERO, true)
        },
    );
    assert_eq!(pose.locomotion.landing, 0.0);
    pose.reset_facing(1.0);
    pose.advance(0.016, motion(Vec3::ZERO, true));
    assert_eq!(pose.locomotion.landing, 0.0);
    pose.advance(
        0.016,
        MotionFrame {
            enabled: false,
            ..motion(Vec3::new(0.0, -10.0, 0.0), false)
        },
    );
    pose.advance(0.016, motion(Vec3::ZERO, true));
    assert_eq!(pose.locomotion.landing, 0.0);
}

fn water(velocity: glam::Vec3) -> MotionFrame {
    MotionFrame {
        medium: MovementMedium::Swimming,
        ..motion(velocity, false)
    }
}

#[test]
fn a_stationary_swimmer_keeps_treading_and_never_becomes_a_fall() {
    let mut pose = BodyPose::default();
    for _ in 0..30 {
        pose.advance(1.0 / 60.0, water(glam::Vec3::ZERO));
    }
    let phase = pose.locomotion.swim.phase;
    pose.advance(1.0 / 60.0, water(glam::Vec3::ZERO));
    assert!(pose.locomotion.swim.weight > 0.9);
    assert_ne!(pose.locomotion.swim.phase, phase);
    assert_eq!(pose.locomotion.airborne, 0.0);
    assert_eq!(pose.walk_weight, 0.0);
    assert_eq!(pose.locomotion.landing, 0.0);
}

#[test]
fn water_entry_cancels_fall_history_and_exit_finishes_the_stroke() {
    use glam::Vec3;
    let mut pose = BodyPose::default();
    pose.advance(0.016, motion(Vec3::new(0.0, -12.0, 0.0), false));
    for _ in 0..60 {
        pose.advance(1.0 / 60.0, water(Vec3::new(0.0, -1.0, 2.5)));
    }
    let before = pose.locomotion.swim;
    pose.advance(0.016, motion(Vec3::ZERO, true));
    let after = pose.locomotion.swim;
    assert!(after.weight > 0.0 && after.weight < before.weight);
    assert_ne!(before.phase, after.phase);
    assert_eq!(pose.locomotion.landing, 0.0);
    for _ in 0..120 {
        pose.advance(1.0 / 60.0, motion(Vec3::ZERO, true));
    }
    assert_eq!(pose.locomotion.swim, Default::default());
}

#[test]
fn swim_direction_blends_without_restarting_the_stroke_and_mounts_clear_it() {
    use glam::Vec3;
    let mut pose = BodyPose::default();
    for _ in 0..60 {
        pose.advance(1.0 / 60.0, water(Vec3::new(0.0, 0.0, 2.5)));
    }
    let phase = pose.locomotion.swim.phase;
    pose.advance(0.016, water(Vec3::new(0.0, 0.0, -2.5)));
    assert!(pose.locomotion.swim.backward > 0.0 && pose.locomotion.swim.backward < 1.0);
    assert!((pose.locomotion.swim.phase - phase).rem_euclid(1.0) < 0.1);
    pose.advance(
        0.016,
        MotionFrame {
            enabled: false,
            ..water(Vec3::ZERO)
        },
    );
    assert_eq!(pose.locomotion.swim, Default::default());
}

#[test]
fn water_floor_contact_walks_then_blends_back_to_swimming() {
    use glam::Vec3;
    let mut pose = BodyPose::default();
    let grounded = || MotionFrame {
        grounded: true,
        ..water(Vec3::new(0.0, 0.0, 1.0))
    };
    for _ in 0..120 {
        pose.advance(1.0 / 60.0, grounded());
    }
    assert!(pose.locomotion.swim.grounded > 0.99);
    assert!(pose.walk_weight > 0.99);
    let phase = pose.anim_time;
    pose.advance(0.016, grounded());
    let wade_step = (pose.anim_time - phase).rem_euclid(1.0);
    let mut land = BodyPose::default();
    land.advance(0.016, motion(grounded().velocity, true));
    assert!(wade_step > 0.0 && wade_step < land.anim_time);
    let floor = pose.locomotion.swim.grounded;
    pose.advance(0.016, water(grounded().velocity));
    assert!(pose.locomotion.swim.grounded > 0.0);
    assert!(pose.locomotion.swim.grounded < floor);
    for _ in 0..120 {
        pose.advance(1.0 / 60.0, water(grounded().velocity));
    }
    assert!(pose.locomotion.swim.grounded < 0.001);
    assert_eq!(pose.walk_weight, 0.0);
    for _ in 0..120 {
        pose.advance(
            1.0 / 60.0,
            MotionFrame {
                velocity: Vec3::ZERO,
                ..grounded()
            },
        );
    }
    assert!(pose.locomotion.swim.grounded > 0.99);
    assert_eq!(pose.walk_weight, 0.0);
    assert_eq!(pose.locomotion.landing, 0.0);
}
