//! Server contracts of the event bus + tick-stage scheduler: the stage seam
//! ordering, same-tick post drains, and the player-death one-shot. Pure
//! server behaviour, so it is tested here against a bare `ServerGame` rather
//! than through a client.

use std::sync::{Arc, Mutex};

use crate::events::tick::TickEvents;
use crate::events::{Attach, DamageSource, PostEvent, PostEventKind, Stage};
use crate::server::game::ServerGame;
use crate::server::session_build::build_server_inline;

#[test]
fn player_died_fires_exactly_once_on_the_zero_transition() {
    let mut server = build_server_inline("", 1, 1);
    let deaths = Arc::new(Mutex::new(0));
    {
        let deaths = deaths.clone();
        server
            .bus
            .on_post(PostEventKind::PlayerDied, 0, move |_, _| {
                *deaths.lock().unwrap() += 1;
            });
    }
    let mut feed = TickEvents::default();
    server.sessions[0].player.set_health(1);
    server.damage_player(0, 2, DamageSource::Fall, None, &mut feed); // 1 → 0: dies
    server.damage_player(0, 2, DamageSource::Fall, None, &mut feed); // already dead: no re-fire
    server.damage_player(0, 0, DamageSource::Fall, None, &mut feed); // the zero fall drain: non-event
    {
        let ServerGame {
            world,
            sessions,
            bus,
            ..
        } = &mut server;
        let sess = &mut sessions[0];
        bus.drain_post(world, &mut sess.player, &mut sess.gui_state, &mut feed);
    }
    assert_eq!(*deaths.lock().unwrap(), 1);
}

#[test]
fn attached_systems_run_in_stage_order_and_post_events_drain_within_the_tick() {
    let mut server = build_server_inline("", 1, 1);
    let log: Arc<Mutex<Vec<&str>>> = Arc::new(Mutex::new(Vec::new()));
    for (label, at) in [
        ("after_spawning", Attach::After(Stage::Spawning)),
        ("before_mining", Attach::Before(Stage::Mining)),
        ("after_mining", Attach::After(Stage::Mining)),
        ("before_mobs", Attach::Before(Stage::Mobs)),
    ] {
        let log = log.clone();
        server
            .systems
            .attach(at, 0, move |_| log.lock().unwrap().push(label));
    }
    {
        // A system's post event must dispatch at the enclosing stage's
        // boundary — within the same tick — not linger to a later tick.
        let log = log.clone();
        server
            .systems
            .attach(Attach::Before(Stage::Placement), 0, move |ctx| {
                log.lock().unwrap().push("emit");
                ctx.queue.emit(PostEvent::PlayerDied);
            });
    }
    {
        let log = log.clone();
        server
            .bus
            .on_post(PostEventKind::PlayerDied, 0, move |_, _| {
                log.lock().unwrap().push("post_handler");
            });
    }
    let mut feed = TickEvents::default();
    server.game_tick_step(&mut feed);
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
