//! Self, remote-player, and environment state rows: HUD reads, player
//! rows, env param shipping, and break-overlay presentation.

use super::common::{game, game_on_empty_chunk};
use super::pump_one_tick;
use crate::events::DamageSource;
use crate::game::presentation::GamePresentationScratch;
use crate::game::tick::{TickEvents, TICK_DT};
use crate::mathh::Vec3;

/// The HUD reads the replicated self view, and after a damage tick's batch it
/// matches session truth exactly.
#[test]
fn hud_health_matches_session_truth_after_a_damage_tick() {
    let mut game = game_on_empty_chunk();

    let mut events = TickEvents::default();
    assert!(game
        .server
        .damage_player(0, 3, DamageSource::Fall, None, &mut events));
    let update = pump_one_tick(&mut game);
    game.apply_tick_update(update);

    let hud = game.player_health().expect("survival draws hearts");
    assert_eq!(
        hud.current,
        game.server.sessions[0].player.health(),
        "the replicated HUD health equals the session's"
    );
    assert_eq!(hud.current, crate::player::MAX_HEALTH - 3);
}

// ---- Remote-player replication ----

/// Every connected session's player row rides every recipient's batch: a
/// second (remote-shaped) session's transform reaches session 0's
/// `TickUpdate`, alongside session 0's own row.
#[test]
fn every_sessions_player_row_reaches_the_local_batch() {
    let mut game = game_on_empty_chunk();
    let s1_pos = Vec3::new(2.5, 64.0, 2.5);
    let s1 = game
        .server
        .add_session_for_test(crate::player::Player::new(s1_pos));
    let s1_id = game.server.sessions[s1].id;

    let update = pump_one_tick(&mut game);
    assert!(
        update
            .players
            .iter()
            .any(|p| p.id == game.server.sessions[0].id),
        "the recipient's own row ships too (the client skips it)"
    );
    let row = update
        .players
        .iter()
        .find(|p| p.id == s1_id)
        .expect("the second session's row rides the first session's batch");
    // Server integrates movement on the tick (F2); without a fresh claim the
    // idle session may fall a little under gravity — still the same session.
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

/// A sleeping session's row carries the server-computed lying head yaw (the
/// bed's base→pillow direction) and flags the tuck teleport as a snap.
#[test]
fn a_sleeping_sessions_row_carries_the_lying_head_yaw() {
    use crate::block::Block;
    use crate::mathh::IVec3;

    let mut game = game_on_empty_chunk();
    for x in 0..16 {
        for z in 0..16 {
            game.server.world.set_block_world(x, 63, z, Block::Stone);
        }
    }
    let base = IVec3::new(7, 64, 7);
    assert!(game.server.world.place_model_block(base, Block::Bed));
    // Night gate: the core day/night system republishes only at tick END
    // (After(Spawning)), so the flag survives until the Placement stage.
    game.server
        .world
        .mod_kv_set("petramond:is_night".into(), vec![1]);
    game.server.sessions[0].look = Some(super::common::hit(base, IVec3::Y));
    game.server.queue_place_click_for_test(0);

    let update = pump_one_tick(&mut game);
    let row = update
        .players
        .iter()
        .find(|p| p.id == game.server.sessions[0].id)
        .expect("own row ships");
    assert!(row.sleeping, "the bed interaction started the sleep");
    let (_, _, cells) = game.server.world.model_group(base).expect("bed group");
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

/// Fix: the day/night shader params are written into the SERVER world's
/// environment, but the renderer reads the CLIENT replica's — the batch must
/// carry them across. Driving full frames, the replica's param map converges
/// on the server's exactly.
#[test]
fn shader_params_replicate_into_the_replica_environment() {
    let mut game = game();
    for _ in 0..3 {
        game.tick(TICK_DT, &crate::game::GameInput::default());
    }
    let server_params = game.server.world.environment().shader_params().clone();
    assert!(
        server_params.contains_key(crate::server::daynight::SKY_TIME_PARAM)
            && server_params.contains_key(crate::server::daynight::SKY_LIGHT_PARAM),
        "day/night published its params server-side"
    );
    let replica_params = game.game.replica.environment().shader_params().clone();
    assert_eq!(
        *replica_params, *server_params,
        "the replica environment mirrors the server's param map"
    );
}

/// The env rides a batch only when the map's VALUES changed since the last
/// shipped copy: the first ticked window ships the full set, a windowless
/// re-read ships `None`, and the next tick (day/night advanced) ships again.
#[test]
fn env_params_ship_on_change_and_none_when_static() {
    let mut game = game();
    let update = pump_one_tick(&mut game);
    let shipped = update
        .env
        .expect("the first batch carries the full param map");
    assert!(
        shipped
            .iter()
            .any(|(k, _)| k == crate::server::daynight::SKY_TIME_PARAM),
        "the day/night keys ride the batch: {shipped:?}"
    );

    let ev = TickEvents::default();
    let quiet = game.server.shared_tick_rows(&ev);
    assert!(
        quiet.env.is_none(),
        "an unchanged param map ships None (keep)"
    );

    let update = pump_one_tick(&mut game);
    assert!(
        update.env.is_some(),
        "the next tick moved the day/night params: the full set ships again"
    );
}

/// Remote break overlays: presentation collects the own crack (replicated
/// self view) PLUS one per visible remote row with a mining target, each at
/// its own stage; an invisible remote (spectator/dead) draws none.
#[test]
fn break_overlays_collect_own_and_visible_remote_miners() {
    use crate::mathh::IVec3;
    use crate::net::protocol::PlayerStateRow;
    use crate::server::player::PlayerId;
    use std::collections::HashMap;

    fn row(id: u8, mining: Option<(IVec3, u8)>, visible: bool) -> PlayerStateRow {
        PlayerStateRow {
            id: PlayerId(id),
            transform: crate::net::protocol::Transform {
                pos: Vec3::new(4.0, 64.0, 4.0),
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
            mining,
            eating: false,
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    let mut game = game();
    game.game.self_view.mining = Some((IVec3::new(1, 64, 1), 4));
    let own_id = game.game.self_id;
    let rows = [
        row(1, Some((IVec3::new(3, 64, 3), 7)), true),
        row(2, Some((IVec3::new(5, 64, 5), 2)), false), // hidden: no overlay
        row(3, None, true),                             // not mining: no overlay
    ];
    game.game
        .remote_players
        .apply(&rows, &[], own_id, &HashMap::new());

    let mut scratch = GamePresentationScratch::new();
    let presentation = scratch.snapshot(&game, 0.0);
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
