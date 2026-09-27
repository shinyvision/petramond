use std::sync::{Arc, Mutex};

use crate::events::tick::TickEvents;
use crate::events::{Attach, DamageSource, PostEvent, PostEventKind, Stage};
use crate::server::session_build::build_server_inline;

#[test]
fn player_died_fires_exactly_once_on_the_zero_transition() {
    let mut server = build_server_inline("", 1, 1);
    let deaths = Arc::new(Mutex::new(0));
    {
        let deaths = deaths.clone();
        server
            .mods
            .bus_mut()
            .on_post(PostEventKind::PlayerDied, 0, move |_, _| {
                *deaths.lock().unwrap() += 1;
            });
    }
    let mut feed = TickEvents::default();
    server.sessions[0].player.set_health(1);
    server.damage_player(0, 2, DamageSource::Fall, None, &mut feed);
    server.damage_player(0, 2, DamageSource::Fall, None, &mut feed);
    server.damage_player(0, 0, DamageSource::Fall, None, &mut feed);
    server.drain_post_events(&mut feed);
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
            .mods
            .systems_mut()
            .attach(at, 0, move |_| log.lock().unwrap().push(label));
    }
    {
        let log = log.clone();
        let player = server.sessions[0].id;
        server
            .mods
            .systems_mut()
            .attach(Attach::Before(Stage::Placement), 0, move |ctx| {
                log.lock().unwrap().push("emit");
                ctx.queue.emit(PostEvent::PlayerDied { player });
            });
    }
    {
        let log = log.clone();
        server
            .mods
            .bus_mut()
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
