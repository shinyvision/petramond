//! Game-level contracts of the Phase 1 event bus + tick-stage scheduler: the
//! stage seam ordering, same-tick post drains, and the player-death one-shot.

use std::sync::{Arc, Mutex};

use super::super::tick::TickEvents;
use super::common::game;
use crate::events::{Attach, DamageSource, PostEvent, PostEventKind, Stage};
use crate::game::{GameInput, ModSpatialSoundCommand};
use crate::mathh::Vec3;

#[test]
fn player_died_fires_exactly_once_on_the_zero_transition() {
    let mut game = game();
    let deaths = Arc::new(Mutex::new(0));
    {
        let deaths = deaths.clone();
        game.server
            .bus
            .on_post(PostEventKind::PlayerDied, 0, move |_, _| {
                *deaths.lock().unwrap() += 1;
            });
    }
    let mut feed = TickEvents::default();
    game.server.sessions[0].player.set_health(3);
    game.server
        .damage_player(0, 2, DamageSource::Fall, None, &mut feed); // 3 → 1: alive
    game.server
        .damage_player(0, 2, DamageSource::Fall, None, &mut feed); // 1 → 0: dies
    game.server
        .damage_player(0, 2, DamageSource::Fall, None, &mut feed); // already dead: no re-fire
    game.server
        .damage_player(0, 0, DamageSource::Fall, None, &mut feed); // the zero fall drain: non-event
    {
        let crate::server::game::ServerGame {
            world,
            sessions,
            bus,
            ..
        } = &mut game.server;
        let sess = &mut sessions[0];
        bus.drain_post(world, &mut sess.player, &mut sess.gui_state, &mut feed);
    }
    assert_eq!(*deaths.lock().unwrap(), 1);
}

#[test]
fn attached_systems_run_in_stage_order_and_post_events_drain_within_the_tick() {
    let mut game = game();
    let log: Arc<Mutex<Vec<&str>>> = Arc::new(Mutex::new(Vec::new()));
    for (label, at) in [
        ("after_spawning", Attach::After(Stage::Spawning)),
        ("before_mining", Attach::Before(Stage::Mining)),
        ("after_mining", Attach::After(Stage::Mining)),
        ("before_mobs", Attach::Before(Stage::Mobs)),
    ] {
        let log = log.clone();
        game.server
            .systems
            .attach(at, 0, move |_| log.lock().unwrap().push(label));
    }
    {
        // A system's post event must dispatch at the enclosing stage's boundary —
        // within the same tick — not linger to a later tick.
        let log = log.clone();
        game.server
            .systems
            .attach(Attach::Before(Stage::Placement), 0, move |ctx| {
                log.lock().unwrap().push("emit");
                ctx.queue.emit(PostEvent::PlayerDied);
            });
    }
    {
        let log = log.clone();
        game.server
            .bus
            .on_post(PostEventKind::PlayerDied, 0, move |_, _| {
                log.lock().unwrap().push("post_handler");
            });
    }
    let mut feed = TickEvents::default();
    game.server.game_tick_step(&mut feed);
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            "before_mining",
            "after_mining",
            "emit",
            "post_handler",
            "before_mobs",
            "after_spawning",
        ]
    );
}

#[test]
fn spatial_sound_commands_reach_game_events_without_loss() {
    let mut game = game();
    let sound = crate::audio::sound_by_name("petramond:item_pickup").expect("engine sound exists");
    game.server
        .systems
        .attach(Attach::Before(Stage::Mining), 0, move |ctx| {
            ctx.feed
                .world
                .spatial_sounds
                .push(ModSpatialSoundCommand::PlayAt {
                    handle: 7,
                    sound,
                    pos: Vec3::new(3.0, 81.0, -2.0),
                    volume: 0.6,
                    pitch: 1.1,
                });
        });

    let events = game.tick(0.05, &GameInput::default());
    assert_eq!(
        events.mod_spatial_sounds,
        vec![ModSpatialSoundCommand::PlayAt {
            handle: 7,
            sound,
            pos: Vec3::new(3.0, 81.0, -2.0),
            volume: 0.6,
            pitch: 1.1,
        }]
    );
}
