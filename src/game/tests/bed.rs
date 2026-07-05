//! Bed behaviour on the tick: sleeping (spawn set, time skip, wake beside the
//! bed), cancelling, and death respawn — at the bed or the surface fallback.

use super::super::bed::SLEEP_TICKS;
use super::super::tick::TickEvents;
use super::super::Game;
use super::common::{game, hit, install_empty_chunk};
use crate::block::Block;
use crate::events::DamageSource;
use crate::mathh::IVec3;
use crate::player::MAX_HEALTH;

const CLOCK_KEY: &str = "llama:clock";

/// A game with a flat stone floor at y=63 and a bed at (7, 64, 7).
fn game_with_bed() -> (Game, IVec3) {
    let mut game = game();
    install_empty_chunk(&mut game);
    for x in 0..16 {
        for z in 0..16 {
            game.world.set_block_world(x, 63, z, Block::Stone);
        }
    }
    let base = IVec3::new(7, 64, 7);
    assert!(game.world.place_model_block(base, Block::Bed));
    (game, base)
}

fn interact_with_bed(game: &mut Game, base: IVec3) -> TickEvents {
    game.look = Some(hit(base, IVec3::Y));
    game.pending_place = true;
    let mut events = TickEvents::default();
    game.tick_place(&mut events);
    events
}

/// Publish the night flag the sleep gate reads (`llama:is_night`); the tests
/// drive tick steps directly, so the day/night system never overwrites it.
fn make_night(game: &mut Game) {
    game.world.mod_kv_set("llama:is_night".into(), vec![1]);
}

fn clock(game: &Game) -> u64 {
    u64::from_le_bytes(
        game.world
            .mod_kv_get(CLOCK_KEY)
            .expect("core day/night publishes a clock")
            .try_into()
            .expect("8-byte LE clock"),
    )
}

fn kill_player(game: &mut Game) -> TickEvents {
    let mut events = TickEvents::default();
    assert!(game.damage_player(MAX_HEALTH, DamageSource::Fall, &mut events));
    assert!(events.player_died, "lethal damage fires the death event");
    events
}

#[test]
fn interacting_with_a_bed_at_night_sets_the_spawn_and_starts_the_sleep() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);

    assert!(game.request_open_sleep, "asks the app for the sleep overlay");
    assert_eq!(game.sleep_progress01(), Some(0.0), "sleep starts at zero");
    let bs = game.player.bed_spawn.expect("one interaction sets the spawn");
    assert_eq!(bs.bed, base);
    assert_ne!(
        (bs.spot.x, bs.spot.z),
        (base.x, base.z),
        "the spawn spot is beside the bed, not on it"
    );
}

#[test]
fn daytime_bed_interaction_sets_the_spawn_but_never_sleeps() {
    let (mut game, base) = game_with_bed();
    // Fresh world = early morning: it is day, so no night flag is set.
    interact_with_bed(&mut game, base);

    assert!(
        game.player.bed_spawn.is_some(),
        "a daytime click still sets the spawn point"
    );
    assert_eq!(game.sleep_progress01(), None, "sleeping is night-only");
    assert!(!game.request_open_sleep, "no sleep overlay by day");
}

#[test]
fn completing_a_sleep_skips_to_morning_and_wakes_beside_the_bed() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);
    let clock_before = clock(&game);

    let mut ended = false;
    for _ in 0..SLEEP_TICKS {
        let mut events = TickEvents::default();
        game.tick_bed_and_respawn(&mut events);
        ended = events.sleep_ended;
    }

    assert!(ended, "the sleep completes after SLEEP_TICKS");
    assert_eq!(game.sleep_progress01(), None, "awake again");
    assert!(
        clock(&game) > clock_before,
        "completing the sleep advances the day clock to the next morning"
    );
    let feet = game.player.pos;
    assert!(
        (feet.x.floor() as i32, feet.z.floor() as i32) != (base.x, base.z),
        "the player wakes beside the bed, not inside it: {feet:?}"
    );
}

#[test]
fn cancelling_a_sleep_wakes_without_skipping_time() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);
    let clock_before = clock(&game);

    game.request_wake();
    let mut events = TickEvents::default();
    game.tick_bed_and_respawn(&mut events);

    assert!(events.sleep_ended, "the cancel ends the sleep on the tick");
    assert_eq!(clock(&game), clock_before, "no time skip on cancel");
    assert!(
        game.player.bed_spawn.is_some(),
        "cancelling keeps the spawn point — one interaction was enough"
    );
}

#[test]
fn damage_while_sleeping_cancels_the_sleep_immediately() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);
    let clock_before = clock(&game);

    let mut events = TickEvents::default();
    assert!(game.damage_player(2, DamageSource::Fall, &mut events));

    assert!(events.sleep_ended, "the hit ends the sleep on the spot");
    assert_eq!(game.sleep_progress01(), None, "no longer sleeping");
    assert_eq!(clock(&game), clock_before, "an interrupted sleep skips no time");
    assert_eq!(game.player.health(), MAX_HEALTH - 2);
    // Woken beside the bed, ready to face the attacker.
    let feet = game.player.pos;
    assert!(
        (feet.x.floor() as i32, feet.z.floor() as i32) != (base.x, base.z),
        "wakes beside the bed: {feet:?}"
    );
}

#[test]
fn respawning_with_a_bed_restores_health_beside_it() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);
    game.request_wake();
    game.tick_bed_and_respawn(&mut TickEvents::default());

    kill_player(&mut game);
    game.request_respawn();
    let mut events = TickEvents::default();
    game.tick_bed_and_respawn(&mut events);

    assert!(events.respawned);
    assert_eq!(game.player.health(), MAX_HEALTH, "respawn restores health");
    let feet = game.player.pos;
    let (dx, dz) = (feet.x - base.x as f32, feet.z - base.z as f32);
    assert!(
        dx.abs() < 8.0 && dz.abs() < 8.0,
        "respawn lands near the bed: {feet:?}"
    );
    assert!(
        (feet.x.floor() as i32, feet.z.floor() as i32) != (base.x, base.z),
        "not inside the bed"
    );
}

#[test]
fn respawn_ignores_requests_while_alive() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);
    game.request_wake();
    game.tick_bed_and_respawn(&mut TickEvents::default());
    let before = game.player.pos;

    game.request_respawn();
    let mut events = TickEvents::default();
    game.tick_bed_and_respawn(&mut events);
    assert!(!events.respawned, "a living player never respawn-teleports");
    assert_eq!(game.player.pos, before);
}

#[test]
fn broken_bed_clears_the_spawn_and_respawn_falls_back_to_the_surface() {
    let (mut game, base) = game_with_bed();
    make_night(&mut game);
    interact_with_bed(&mut game, base);
    game.request_wake();
    game.tick_bed_and_respawn(&mut TickEvents::default());

    // The break path resolves the spawn clear before removal (breaking.rs);
    // this drives the same hook + removal pair it uses.
    game.clear_bed_spawn_at(base);
    game.world.remove_model_block(base);
    assert!(
        game.player.bed_spawn.is_none(),
        "destroying the bed removes the respawn point"
    );

    kill_player(&mut game);
    game.request_respawn();
    let mut events = TickEvents::default();
    game.tick_bed_and_respawn(&mut events);

    assert!(events.respawned);
    assert_eq!(game.player.health(), MAX_HEALTH);
    // The fallback is the fresh-world pick: a random dry-land column within
    // 500 blocks of the origin (plus the block-centre offset).
    let feet = game.player.pos;
    let dist_sq = feet.x * feet.x + feet.z * feet.z;
    assert!(
        dist_sq <= 501.0 * 501.0,
        "surface fallback stays within the spawn search radius: {feet:?}"
    );
}
