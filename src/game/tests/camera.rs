use super::common::game;
use crate::mathh::Vec3;

#[test]
fn camera_eases_grounded_step_up_to_the_player_eye() {
    let mut game = game();
    game.player.pos = Vec3::new(0.0, 64.0, 0.0);
    game.player.vel = Vec3::ZERO;
    game.player.on_ground = true;
    game.sync_camera_to_player_eye(1.0 / 60.0);

    let old_eye_y = game.player.eye().y;
    let stepped_feet_y = game.player.pos.y + crate::collision::STEP_HEIGHT;
    game.player.pos.y = stepped_feet_y;
    game.player.vel.y = 0.0;
    game.player.on_ground = true;
    game.sync_camera_to_player_eye(1.0 / 60.0);

    let target_eye_y = game.player.eye().y;
    assert_eq!(game.player.pos.y, stepped_feet_y);
    assert!(
        game.cam.pos.y > old_eye_y && game.cam.pos.y < target_eye_y,
        "camera should ease upward after a grounded step: old={old_eye_y}, cam={}, target={target_eye_y}",
        game.cam.pos.y
    );

    for _ in 0..60 {
        game.sync_camera_to_player_eye(1.0 / 60.0);
    }
    assert!(
        (game.cam.pos.y - target_eye_y).abs() < 0.002,
        "camera should settle back to the eye: cam={}, target={target_eye_y}",
        game.cam.pos.y
    );
}
