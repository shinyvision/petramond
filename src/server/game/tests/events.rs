use std::sync::{Arc, Mutex};

use crate::events::tick::TickEvents;
use crate::events::{Attach, DamageSource, PostEvent, PostEventKind, Stage};
use crate::net::protocol::ClientToServer;
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

type ClientEventLog = Arc<Mutex<Vec<(crate::player::PlayerId, String, Vec<u8>)>>>;

/// A server whose only mod is `fixture`, with every queued client event recorded as the bus
/// drains it.
fn server_logging_client_events() -> (crate::server::game::ServerGame, ClientEventLog) {
    let mut server = build_server_inline("", 1, 1);
    server
        .mods
        .replace_host(crate::modding::ModHost::test_unit_guest_host("fixture"));
    let log = ClientEventLog::default();
    {
        let log = log.clone();
        server
            .mods
            .bus_mut()
            .on_post(PostEventKind::ClientEvent, 0, move |_, ev| {
                if let PostEvent::ClientEvent { player, key, data } = ev {
                    log.lock()
                        .unwrap()
                        .push((*player, key.clone(), data.clone()));
                }
            });
    }
    (server, log)
}

fn wire_mod_event(key: &str, data: Vec<u8>) -> ClientToServer {
    ClientToServer::ModEvent {
        key: key.into(),
        data,
    }
}

#[test]
fn a_wire_mod_event_is_trusted_only_for_a_session_mods_key_and_a_bounded_payload() {
    let (mut server, log) = server_logging_client_events();
    let sender = server.sessions[0].id;
    let mut inbox = vec![
        wire_mod_event("other:picked", vec![1]),
        wire_mod_event("fixture", vec![1]),
        wire_mod_event("fixture:", vec![1]),
        wire_mod_event(":picked", vec![1]),
        wire_mod_event("fixture:big", vec![0; mod_api::EVENT_MAX_DATA_BYTES + 1]),
        wire_mod_event("fixture:picked", vec![7]),
    ];
    server.pump(0.06, &mut inbox);
    assert_eq!(
        *log.lock().unwrap(),
        [(sender, "fixture:picked".to_owned(), vec![7])]
    );
}

#[test]
fn a_flood_of_wire_mod_events_is_cut_at_the_session_budget() {
    use crate::server::player::MOD_EVENT_BURST;

    let (mut server, log) = server_logging_client_events();
    let burst = MOD_EVENT_BURST as usize;
    let mut inbox: Vec<_> = (0..burst * 4)
        .map(|_| wire_mod_event("fixture:picked", Vec::new()))
        .collect();
    server.pump(0.06, &mut inbox);
    let heard = log.lock().unwrap().len();
    assert!(
        (burst..burst * 4).contains(&heard),
        "{heard} of {} events got through a budget of {burst}",
        burst * 4
    );
}

#[test]
fn wire_mod_events_dispatch_in_send_order_before_the_ticks_menu_input() {
    let (mut server, log) = server_logging_client_events();
    {
        let log = log.clone();
        let player = server.sessions[0].id;
        server
            .mods
            .systems_mut()
            .attach(Attach::Before(Stage::Menu), 0, move |_| {
                log.lock()
                    .unwrap()
                    .push((player, "menu stage".into(), Vec::new()));
            });
    }
    let mut inbox = vec![
        wire_mod_event("fixture:first", Vec::new()),
        ClientToServer::CraftRecipe {
            recipe: "fixture:none".into(),
            bulk: false,
            request_id: 1,
        },
        wire_mod_event("fixture:second", Vec::new()),
    ];
    server.pump(0.06, &mut inbox);
    let order: Vec<String> = log.lock().unwrap().iter().map(|e| e.1.clone()).collect();
    assert_eq!(order, ["fixture:first", "fixture:second", "menu stage"]);
}
