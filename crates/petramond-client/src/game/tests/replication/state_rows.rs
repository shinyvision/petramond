use super::common::{game, game_on_empty_chunk};
use super::pump_one_tick;
use crate::game::presentation::GamePresentationScratch;
use petramond::events::tick::{TickEvents, TICK_DT};
use petramond::events::DamageSource;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

#[test]
fn replicated_conditions_draw_on_local_and_remote_players_without_hud_effects() {
    let mut game = game_on_empty_chunk();
    let remote = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(WorldPos::new(
            4.5, 65.0, 4.5,
        )));
    let mut scratch = GamePresentationScratch::new();
    let view = petramond_render::camera::ViewVolume::unbounded();
    let baseline = scratch.snapshot(&game, 0.0, &view).particle_emitters.len();
    let burning = petramond_world::condition::by_name("petramond:burning").unwrap();
    let strongest = (burning.def().stages.len() - 1) as u8;
    let bundle = petramond_world::particle_emitters::by_key(
        burning.def().stages[strongest as usize].emitter.unwrap(),
    )
    .unwrap();
    for s in [0, remote] {
        game.session_at_mut(s)
            .player_mut()
            .exposure_mut()
            .apply(burning.def(), strongest, 80);
    }
    let events = TickEvents::default();
    let shared = game.sim_mut().shared_tick_rows(&events);
    let update = game.sim_mut().build_tick_update(0, &events, &shared);
    game.apply_tick_update(Box::new(update));
    game.commit_replication_window_for_test();
    assert!(game.player_effect_icons().is_empty());
    let presentation = scratch.snapshot(&game, 0.0, &view);
    assert_eq!(
        presentation.particle_emitters.len(),
        baseline + bundle.rows.len() * 2,
        "first-person fire and the visible remote each contribute their flames"
    );
    assert_eq!(
        presentation.bodies[0].body.emitter_tint,
        bundle.tint.unwrap_or([1.0; 3])
    );
    assert_eq!(
        presentation.bodies[0].body.emitter_self_lit,
        bundle.body_self_lit
    );

    game.toggle_third_person();
    game.set_particles_mode(petramond::save::client::ParticlesMode::Off);
    let presentation = scratch.snapshot(&game, 0.0, &view);
    assert!(presentation.particle_emitters.is_empty());
    assert_eq!(
        presentation.bodies[0].body.emitter_self_lit,
        bundle.body_self_lit
    );
    assert_eq!(
        presentation.bodies[1].body.emitter_self_lit,
        bundle.body_self_lit
    );
    game.set_particles_mode(petramond::save::client::ParticlesMode::Full);

    for s in [0, remote] {
        game.session_at_mut(s).player_mut().clear_exposure();
    }
    let shared = game.sim_mut().shared_tick_rows(&events);
    let update = game.sim_mut().build_tick_update(0, &events, &shared);
    game.apply_tick_update(Box::new(update));
    game.commit_replication_window_for_test();
    let presentation = scratch.snapshot(&game, 0.0, &view);
    assert_eq!(
        presentation.particle_emitters.len(),
        baseline,
        "extinguishing removes both bodies' flames"
    );
    assert_eq!(presentation.bodies[1].body.emitter_tint, [1.0; 3]);
    assert_eq!(presentation.bodies[1].body.emitter_self_lit, 0.0);
    assert_eq!(presentation.bodies[0].body.emitter_self_lit, 0.0);
}

#[test]
fn hud_health_matches_session_truth_after_a_damage_tick() {
    let mut game = game_on_empty_chunk();

    let mut events = TickEvents::default();
    assert!(game
        .sim_mut()
        .damage_player(0, 3, DamageSource::Fall, None, &mut events));
    let update = pump_one_tick(&mut game);
    game.apply_tick_update(update);

    let hud = game.player_health().expect("survival draws hearts");
    assert_eq!(
        hud.current,
        game.server_player().health(),
        "the replicated HUD health equals the session's"
    );
    assert_eq!(hud.current, petramond::player::MAX_HEALTH - 3);
}

#[test]
fn every_sessions_player_row_reaches_the_local_batch() {
    let mut game = game_on_empty_chunk();
    let s1_pos = WorldPos::new(2.5, 64.0, 2.5);
    let s1 = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(s1_pos));
    let s1_id = game.session_at(s1).id();

    let update = pump_one_tick(&mut game);
    assert!(
        update
            .players()
            .expect("players section")
            .iter()
            .any(|p| p.id == game.session().id()),
        "the recipient's own row ships too (the client skips it)"
    );
    let row = update
        .players()
        .expect("players section")
        .iter()
        .find(|p| p.id == s1_id)
        .expect("the second session's row rides the first session's batch");
    assert!(
        (row.transform.pos - s1_pos).length() < 1.0,
        "second session stays near its spawn (got {:?}, want near {:?})",
        row.transform.pos,
        s1_pos
    );
    assert!(row.alive && row.visible);
    assert!(!row.sleeping && row.sleep_yaw.is_none());
    assert!(
        !row.snap,
        "idle gravity is not a teleport snap for observers"
    );
}

#[test]
fn a_sleeping_sessions_row_carries_the_lying_head_yaw() {
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;

    let mut game = game_on_empty_chunk();
    for x in 0..16 {
        for z in 0..16 {
            game.server_world_mut()
                .set_block_world(x, 63, z, Block::Stone);
        }
    }
    let base = IVec3::new(7, 64, 7);
    assert!(game.server_world_mut().place_model_block(base, Block::Bed));
    game.server_player_mut().pos = WorldPos::new(3.5, 64.0, 7.5);
    game.server_world_mut()
        .world_kv_set("petramond:is_night".into(), vec![1]);
    game.session_mut().input_mut().look = Some(super::common::hit(base, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);

    let update = pump_one_tick(&mut game);
    let row = update
        .players()
        .expect("players section")
        .iter()
        .find(|p| p.id == game.session().id())
        .expect("own row ships");
    assert!(row.sleeping, "the bed interaction started the sleep");
    let (_, _, cells) = game.server_world().model_group(base).expect("bed group");
    let other = cells
        .iter()
        .copied()
        .find(|c| *c != base)
        .expect("two-cell bed");
    let d = other - base;
    assert_eq!(
        row.sleep_yaw,
        Some((d.x as f32).atan2(d.z as f32)),
        "the lying head yaw points from the bed base toward the pillow"
    );
    assert!(row.snap, "the tuck teleport must snap interpolation");
}

#[test]
fn shader_params_replicate_into_the_replica_environment() {
    let mut game = game();
    for _ in 0..3 {
        game.tick(TICK_DT, &crate::game::GameInput::default());
    }
    let server_params = game
        .server_world()
        .data()
        .environment()
        .shader_params()
        .clone();
    assert!(
        server_params.contains_key(petramond::rules::daynight::SKY_TIME_PARAM)
            && server_params.contains_key(petramond::rules::daynight::SKY_LIGHT_PARAM),
        "day/night published its params server-side"
    );
    let replica_params = game
        .game
        .replica
        .world
        .data()
        .environment()
        .shader_params()
        .clone();
    assert_eq!(
        *replica_params, *server_params,
        "the replica environment mirrors the server's param map"
    );
}

#[test]
fn env_params_ship_on_change_and_none_when_static() {
    let mut game = game();
    let update = pump_one_tick(&mut game);
    let shipped = update
        .env()
        .cloned()
        .expect("the first batch carries the full param map");
    assert!(
        shipped
            .iter()
            .any(|(k, _)| k == petramond::rules::daynight::SKY_TIME_PARAM),
        "the day/night keys ride the batch: {shipped:?}"
    );

    let ev = TickEvents::default();
    let quiet = game.sim_mut().shared_tick_rows(&ev);
    assert!(
        quiet.env.is_none(),
        "an unchanged param map ships None (keep)"
    );

    let update = pump_one_tick(&mut game);
    assert!(
        update.env().is_some(),
        "the next tick moved the day/night params: the full set ships again"
    );
}

#[test]
fn break_overlays_collect_own_and_visible_remote_miners() {
    use petramond::net::protocol::PlayerStateRow;
    use petramond::player::PlayerId;
    use petramond_math::math::IVec3;

    fn row(id: u8, mining: Option<(IVec3, u8)>, visible: bool) -> PlayerStateRow {
        PlayerStateRow {
            conditions: Vec::new(),
            id: PlayerId(id),
            transform: petramond::net::protocol::Transform {
                pos: WorldPos::new(4.0, 64.0, 4.0),
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            on_ground: true,
            sneaking: false,
            sleeping: false,
            sleep_yaw: None,
            alive: visible,
            visible,
            held_item: None,
            held_data: None,
            off_hand_item: None,
            off_hand_data: None,
            mining,
            eating: false,
            eating_off_hand: false,
            held_pose_main: None,
            held_pose_off: None,
            held_display: [None; 2],
            bone_poses: Vec::new(),
            animator: Default::default(),
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    let mut game = game();
    game.game.replica.self_view.mining = Some((IVec3::new(1, 64, 1), 4));
    let own_id = game.game.replica.entities.self_id();
    let rows = [
        row(1, Some((IVec3::new(3, 64, 3), 7)), true),
        row(2, Some((IVec3::new(5, 64, 5), 2)), false),
        row(3, None, true),
    ];
    game.game
        .replica
        .entities
        .players_mut()
        .apply_snapshot(&rows, &[], own_id);

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(
        &game,
        0.0,
        &petramond_render::camera::ViewVolume::unbounded(),
    );
    let overlays = presentation.break_overlays;
    assert_eq!(overlays.len(), 2, "own + the one visible remote miner");
    assert!(
        overlays
            .iter()
            .any(|o| o.block == IVec3::new(1, 64, 1) && o.stage == 4),
        "the own overlay keeps its target + stage"
    );
    assert!(
        overlays
            .iter()
            .any(|o| o.block == IVec3::new(3, 64, 3) && o.stage == 7),
        "the remote row's overlay carries ITS stage"
    );
}
