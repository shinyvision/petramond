use super::common::game;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

#[test]
fn camera_eases_grounded_step_up_to_the_player_eye() {
    // The camera mirrors the CLIENT's predicted player.
    let mut game = game();
    game.local.player.pos = WorldPos::new(0.0, 64.0, 0.0);
    game.local.player.vel = Vec3::ZERO;
    game.local.player.on_ground = true;
    game.sync_camera_to_player_eye(1.0 / 60.0);

    let old_eye_y = game.local.player.eye().y;
    let stepped_feet_y =
        game.local.player.pos.y + f64::from(petramond_world::collision::STEP_HEIGHT);
    game.local.player.pos.y = stepped_feet_y;
    game.local.player.vel.y = 0.0;
    game.local.player.on_ground = true;
    game.sync_camera_to_player_eye(1.0 / 60.0);

    let target_eye_y = game.local.player.eye().y;
    assert_eq!(game.local.player.pos.y, stepped_feet_y);
    assert!(
        game.local.cam.pos.y > old_eye_y && game.local.cam.pos.y < target_eye_y,
        "camera should ease upward after a grounded step: old={old_eye_y}, cam={}, target={target_eye_y}",
        game.local.cam.pos.y
    );

    for _ in 0..60 {
        game.sync_camera_to_player_eye(1.0 / 60.0);
    }
    assert!(
        (game.local.cam.pos.y - target_eye_y).abs() < 0.002,
        "camera should settle back to the eye: cam={}, target={target_eye_y}",
        game.local.cam.pos.y
    );
}

/// View bob sways the FIRST-PERSON eye and must never reach the third-person
/// boom — which is `self.cam` cloned and retreated, so the only thing keeping
/// it out is the suppression in `sync_camera_to_player_eye`. Nothing else in
/// the frame would show a bobbing boom.
#[test]
fn view_bob_sways_the_first_person_eye_and_never_the_third_person_boom() {
    let mut game = game();
    game.local.player.pos = WorldPos::new(0.0, 64.0, 0.0);
    game.local.player.on_ground = true;
    game.local.player.yaw = 0.0;
    game.local.cam.yaw = 0.0;

    // Two seconds of walking: with yaw 0 the sway rides +X, and the rise is the
    // camera's departure from the eye height.
    let walk = |game: &mut crate::game::Game, frames: usize| -> (f32, f32) {
        let (mut sway, mut rise) = (0.0f32, 0.0f32);
        for _ in 0..frames {
            game.local.player.vel = Vec3::new(4.3, 0.0, 0.0);
            game.sync_camera_to_player_eye(1.0 / 60.0);
            sway = sway.max(((game.local.cam.pos.x - game.local.player.pos.x).abs()) as f32);
            rise = rise.max(((game.local.cam.pos.y - game.local.player.eye().y).abs()) as f32);
        }
        (sway, rise)
    };

    let (sway, rise) = walk(&mut game, 120);
    assert!(sway > 0.01, "first person should sway sideways: {sway}");
    assert!(rise > 0.005, "…and rise/dip a little: {rise}");

    // Switching to third person settles the eye back onto the player's axis,
    // so the boom cloned from it carries no sway at all.
    game.local.third_person.enabled = true;
    walk(&mut game, 120);
    let (sway, rise) = walk(&mut game, 120);
    assert!(sway < 1e-4, "third person must not sway: {sway}");
    assert!(rise < 1e-4, "third person must not rise: {rise}");
}

/// A mount carries the body up a slope; that rise is not a step. The glide
/// that eases a stepped-up body is a NEGATIVE lag, so letting it run on a
/// seated body draws the rider (and the first-person eye) under the seat on
/// every climb — and only on climbs.
#[test]
fn a_seat_rising_up_a_slope_is_not_a_step_the_body_glides_behind() {
    let mut game = game();
    game.local.player.pos = WorldPos::new(0.0, 64.0, 0.0);
    game.local.player.vel = Vec3::ZERO;
    game.local.player.on_ground = true;
    game.sync_camera_to_player_eye(1.0 / 60.0);
    let pos = game.local.player.pos;
    game.replica
        .entities
        .set_own_mount(Some(petramond::net::protocol::PlayerMount::Anchor {
            pos,
            yaw: 0.0,
            pose: 0,
        }));
    for _ in 0..30 {
        // A 45° climb at a cart's pace, one frame at a time: each frame's
        // rise is well inside the step height the glide would ease.
        game.local.player.pos.y += 0.05;
        game.sync_camera_to_player_eye(1.0 / 60.0);
        assert_eq!(
            game.local.camera_rig.step_y_offset(),
            0.0,
            "a carried body never lags its seat"
        );
    }
}
