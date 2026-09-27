//! The client mods' view claims as the frame presents them: the camera the
//! frame draws from and hears from, the perspective over the player's own key,
//! and the shader-param overrides over the replicated environment.

use super::common::game;
use petramond::modding::client::view::{ViewCameraClaim, ViewFold};
use petramond_math::world_pos::WorldPos;

fn perspective(third_person: Option<bool>) -> ViewFold {
    ViewFold {
        third_person,
        ..Default::default()
    }
}

#[test]
fn a_perspective_claim_holds_over_the_key_and_release_restores_the_players_own() {
    let mut game = game();
    game.toggle_third_person();
    assert!(game.third_person_enabled());

    game.apply_view_fold(perspective(Some(false)));
    assert!(!game.third_person_enabled());
    assert_eq!(
        game.render_camera().pos,
        game.local.cam.pos,
        "first person is the eye"
    );
    // The key does nothing while the claim stands...
    game.toggle_third_person();
    assert!(!game.third_person_enabled());

    // ...so release presents exactly the mode the player left, boom placed on
    // the release frame itself.
    game.apply_view_fold(ViewFold::default());
    assert!(game.third_person_enabled());
    assert!(game.local.third_person.cam.is_some());
    assert!(game.presents_local_body());

    game.toggle_third_person();
    game.apply_view_fold(perspective(Some(true)));
    assert!(game.third_person_enabled() && game.presents_local_body());
    game.apply_view_fold(ViewFold::default());
    assert!(!game.third_person_enabled());
    assert_eq!(game.render_camera().pos, game.local.cam.pos);
}

#[test]
fn a_claimed_camera_is_the_frames_camera_and_listener_until_released() {
    let mut game = game();
    let claim = ViewCameraClaim {
        pos: [10.5, 90.0, -3.25],
        yaw: 1.0,
        pitch: -0.3,
        roll: 0.2,
        fov_y: None,
        anchor: None,
    };
    let claimed = ViewFold {
        camera: Some(claim),
        ..Default::default()
    };
    game.apply_view_fold(claimed.clone());
    {
        let frame = game.client_frame(0.0);
        assert_eq!(frame.camera.pos, WorldPos::new(10.5, 90.0, -3.25));
        assert_eq!(
            (frame.camera.yaw, frame.camera.pitch, frame.camera.roll),
            (1.0, -0.3, 0.2)
        );
        assert_eq!(frame.listener().pos, frame.camera.pos);
    }
    assert!(game.camera_claimed());
    // The camera is not the eye, so the body presents under it — unless the
    // player is a spectator, who has no body to present.
    assert!(game.presents_local_body());
    let mode = game.local.player.mode();
    game.local
        .player
        .set_mode(petramond::player::PlayerMode::Spectator);
    assert!(!game.presents_local_body());
    game.local.player.set_mode(mode);
    assert_ne!(
        game.local.cam.pos,
        game.render_camera().pos,
        "the sim eye never moves"
    );

    // No `fov_y`: the player's LIVE field of view, not the one at claim time.
    game.local.cam.fov_y = 1.3;
    game.apply_view_fold(claimed);
    assert_eq!(game.render_camera().fov_y, 1.3);

    game.apply_view_fold(ViewFold::default());
    assert!(!game.camera_claimed() && !game.presents_local_body());
    let frame = game.client_frame(0.0);
    assert_eq!(frame.camera.pos, game.local.cam.pos);
    assert_eq!(frame.listener().pos, game.local.cam.pos);
}

#[test]
fn a_time_override_scrubs_the_sky_through_the_one_sky_model_and_clears_back() {
    use petramond::rules::daynight::{sky_params, SKY_LIGHT_PARAM, SKY_TIME_PARAM};
    let mut game = game();
    let replicated = (*game.client_frame(0.0).environment.shader_params).clone();

    let mut fold = ViewFold::default();
    fold.env.insert(SKY_TIME_PARAM.into(), [0.8, 0.0, 3.0, 0.0]);
    fold.env.insert("test:tint".into(), [0.5; 4]);
    game.apply_view_fold(fold.clone());
    let params = game.client_frame(0.0).environment.shader_params;
    let (time, light) = sky_params(0.8, 3.0);
    assert_eq!(params.get(SKY_TIME_PARAM), Some(&time));
    assert_eq!(params.get(SKY_LIGHT_PARAM), Some(&light));
    assert_eq!(params.get("test:tint"), Some(&[0.5; 4]));

    // A light override of its own beats the derived one.
    fold.env.insert(SKY_LIGHT_PARAM.into(), [0.25; 4]);
    game.apply_view_fold(fold);
    let params = game.client_frame(0.0).environment.shader_params;
    assert_eq!(params.get(SKY_LIGHT_PARAM), Some(&[0.25; 4]));

    game.apply_view_fold(ViewFold::default());
    assert_eq!(
        *game.client_frame(0.0).environment.shader_params,
        replicated
    );
}

/// Seed one remote player standing at `feet` into the replica, as a batch
/// delivers it (within the local player's interest).
fn with_remote(game: &mut super::common::TestGame, feet: WorldPos) -> mod_api::PlayerId {
    use petramond::events::tick::TickEvents;
    let index = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(feet));
    let id = game.session_at(index).id();
    let events = TickEvents::default();
    let shared = game.sim_mut().shared_tick_rows(&events);
    let update = game.sim_mut().build_tick_update(0, &events, &shared);
    game.apply_tick_update(Box::new(update));
    game.commit_replication_window_for_test();
    mod_api::PlayerId(id.0)
}

/// Another player's view presents from their eye (their own body hidden,
/// their hands in the viewmodel), a camera anchored to them follows their
/// presented feet, and the rows a mod reads describe them as drawn.
#[test]
fn a_subject_and_an_anchor_present_from_another_players_body() {
    let mut game = game();
    // Beside the local player: a batch carries only the players in its
    // interest.
    let here = game.local.player.pos;
    let feet = WorldPos::new(here.x + 4.0, here.y, here.z + 4.0);
    let subject = with_remote(&mut game, feet);
    game.publish_presented_entities();
    let rows = game.presented_entities();
    let row = rows
        .iter()
        .find(|row| row.id == mod_api::EntityRef::Player(subject))
        .expect("the remote is in the frame");
    assert!((row.feet[1] - feet.y).abs() < 1e-6);
    assert!((row.eye[1] - feet.y - f64::from(petramond::player::EYE)).abs() < 1e-5);

    game.apply_view_fold(ViewFold {
        subject: Some(subject),
        ..Default::default()
    });
    assert_eq!(game.view_subject(), Some(subject));
    let eye = game.render_camera().pos;
    assert!((eye.x - feet.x).abs() < 1e-6 && (eye.z - feet.z).abs() < 1e-6);
    assert_eq!(
        game.hidden_remote_body(),
        Some(petramond::player::PlayerId(subject.0)),
        "the subject's own body is not drawn around the eye"
    );
    assert!(game.subject_hands().is_some());

    let anchored = ViewCameraClaim {
        pos: [0.0, 3.0, -2.0],
        yaw: 0.0,
        pitch: 0.0,
        roll: 0.0,
        fov_y: None,
        anchor: Some(mod_api::EntityRef::Player(subject)),
    };
    game.apply_view_fold(ViewFold {
        camera: Some(anchored),
        ..Default::default()
    });
    let cam = game.render_camera().pos;
    assert!((cam.y - (feet.y + 3.0)).abs() < 1e-6 && (cam.z - (feet.z - 2.0)).abs() < 1e-6);
    assert!(!game.camera_anchor_missing());

    game.apply_view_fold(ViewFold {
        camera: Some(ViewCameraClaim {
            anchor: Some(mod_api::EntityRef::Mob(u64::MAX)),
            ..anchored
        }),
        ..Default::default()
    });
    assert!(game.camera_anchor_missing(), "an absent anchor says so");
}
