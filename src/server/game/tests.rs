use crate::net::protocol::ServerToClient;
use crate::server::chat::ChatTargets;

mod events;
mod interest;

fn chat_texts(msgs: &[ServerToClient]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| match m {
            ServerToClient::ChatLine(line) => Some(
                line.spans
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

#[test]
fn targeted_chat_reaches_only_listed_sessions() {
    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let remote_s = server.add_session_for_test(player);
    let remote_id = server.sessions[remote_s].id;

    server
        .chat
        .authored("only-remote", ChatTargets::Players(vec![remote_id]));
    server.chat.authored("everyone", ChatTargets::All);

    let out = server.pump(0.0, &mut Vec::new());
    let local = chat_texts(&out.msgs);
    assert!(
        !local.iter().any(|t| t.contains("only-remote")),
        "local must not receive a remote-only line"
    );
    assert!(
        local.iter().any(|t| t.contains("everyone")),
        "local must receive broadcast"
    );

    let remote_msgs = out
        .remote
        .iter()
        .find(|(id, _)| *id == remote_id)
        .map(|(_, msgs)| msgs.as_slice())
        .unwrap_or(&[]);
    let remote = chat_texts(remote_msgs);
    assert!(
        remote.iter().any(|t| t.contains("only-remote")),
        "remote must receive its targeted line"
    );
    assert!(
        remote.iter().any(|t| t.contains("everyone")),
        "remote must receive broadcast"
    );
}

#[test]
fn a_published_body_claim_reaches_the_addressed_sessions_movement() {
    use crate::player::MOVE_SCALE_DEFAULT;

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let other = crate::server::session_build::spawn_player(server.world.data().seed);
    let s = server.add_session_for_test(other);
    assert_ne!(s, 0, "the claimed body must not be the host session");

    let claimed = &mut server.sessions[s].player;
    assert!(claimed
        .claims
        .set_attribute("alpha", mod_api::PlayerAttribute::MoveSpeed, 0.5));
    assert!(claimed
        .claims
        .set_attribute("beta", mod_api::PlayerAttribute::MoveSpeed, 0.5));
    assert_eq!(claimed.move_scale(), 0.25, "two packs' claims multiply");
    assert_eq!(
        server.sessions[0].player.move_scale(),
        MOVE_SCALE_DEFAULT,
        "a claim on one body must not reach another"
    );

    let walk = crate::player::Input {
        wishdir: petramond_math::math::Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let full = server.sessions[0].player.wish_speed(walk);
    assert_eq!(server.sessions[s].player.wish_speed(walk), full * 0.25);

    server.sessions[s].player.claims.clear();
    assert_eq!(server.sessions[s].player.wish_speed(walk), full);
}

#[test]
fn a_cancelled_pre_damage_applies_neither_damage_nor_its_knockback() {
    use crate::events::{tick::TickEvents, DamageSource, Outcome};

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    server.sessions[0].input.intent_gameplay = true;
    server.sessions[0].input.intent_use_held = true;
    server.publish_player_inputs();

    server
        .mods
        .bus_mut()
        .on_player_damage_pre(0, move |ctx, _ev| {
            let holding_use = ctx
                .actor
                .and_then(|id| {
                    ctx.world
                        .player_roster()
                        .iter()
                        .find(|r| r.id == id.0)
                        .map(|r| r.use_held)
                })
                .unwrap_or(false);
            if holding_use {
                Outcome::Cancel
            } else {
                Outcome::Continue
            }
        });

    let before = server.sessions[0].player.pos;
    let hit = |server: &mut crate::server::game::ServerGame| {
        server.sessions[0].player.set_health(20);
        server.sessions[0].player.clear_damage_immunity();
        server.publish_player_inputs();
        server.damage_player(
            0,
            4,
            DamageSource::Fall,
            Some(before + petramond_math::math::Vec3::new(-2.0, 0.0, 0.0)),
            &mut TickEvents::default(),
        )
    };

    assert!(!hit(&mut server), "the handler cancelled the strike");
    assert_eq!(server.sessions[0].player.health(), 20, "no damage applied");
    assert_eq!(
        server.sessions[0].player.vel,
        petramond_math::math::Vec3::ZERO,
        "a cancelled hit must not knock the body back either"
    );

    server.sessions[0].input.intent_use_held = false;
    assert!(hit(&mut server), "an unguarded strike must land");
    assert_eq!(server.sessions[0].player.health(), 16);
}

#[test]
fn a_mod_cue_reaches_only_the_session_it_names_and_never_coalesces() {
    use crate::events::{tick::TickEvents, ClientEvent};

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let other_s = server.add_session_for_test(player);
    let other_id = server.sessions[other_s].id;
    assert_ne!(other_s, 0);

    let mut events = TickEvents::default();
    for data in [vec![7], vec![8]] {
        events.client_events.push(ClientEvent {
            player: other_id,
            key: "alpha:cue".into(),
            data,
        });
    }

    let shared = server.shared_tick_rows(&events);
    let batch = |server: &mut crate::server::game::ServerGame, s| {
        server
            .build_tick_update(s, &events, &shared)
            .self_events()
            .map(|e| e.client_events.clone())
            .unwrap_or_default()
    };
    assert!(
        batch(&mut server, 0).is_empty(),
        "not the addressed session"
    );
    assert_eq!(
        batch(&mut server, other_s)
            .iter()
            .map(|m| (m.key.as_str(), m.data.clone()))
            .collect::<Vec<_>>(),
        [("alpha:cue", vec![7]), ("alpha:cue", vec![8])],
    );
}

#[test]
fn a_denied_body_cannot_swing_or_run_its_mining_timer() {
    use crate::events::tick::TickEvents;
    use crate::player::DeniedActions;
    use mod_api::BodyAction::{Attack, Mine};
    use petramond_math::math::IVec3;

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    server.sessions[0].input.intent_gameplay = true;

    let feet = server.sessions[0].player.pos;
    let cell = IVec3::new(
        feet.x.floor() as i32,
        feet.y.floor() as i32 - 1,
        feet.z.floor() as i32,
    );
    server
        .world
        .set_block_world(cell.x, cell.y, cell.z, petramond_world::block::Block::Stone);
    server.sessions[0].input.look = Some(crate::net::protocol::TargetRef::face(
        cell,
        IVec3::new(0, 1, 0),
    ));
    server.sessions[0].input.intent_break_held = true;

    let mut events = TickEvents::default();
    server.tick_mining(0, &mut events);
    assert_eq!(
        server.sessions[0].sim.mining.overlay().map(|(p, _)| p),
        Some(cell),
        "an unclaimed body mines what it is looking at"
    );

    server.sessions[0]
        .player
        .claims
        .set_denied_actions("combat", DeniedActions::of([Attack, Mine]));
    server.tick_mining(0, &mut events);
    assert!(
        server.sessions[0].sim.mining.overlay().is_none(),
        "a denied mine RESETS the timer, it does not pause it"
    );

    server.sessions[0].input.look = None;
    server.sessions[0].input.latch_attack(Default::default());
    server.tick_attack(0, &mut events);
    assert!(!events.player_at(0).swung_hand, "no swing while denied");
    assert_eq!(
        server.sessions[0].sim.attack_cooldown, 0,
        "a denied swing arms no cooldown — it did not happen"
    );

    server.sessions[0].player.claims.clear();
    server.tick_attack(0, &mut events);
    assert!(
        !events.player_at(0).swung_hand,
        "and the denied press was SPENT, not stored for the release"
    );

    server.sessions[0].input.latch_attack(Default::default());
    server.tick_attack(0, &mut events);
    assert!(events.player_at(0).swung_hand);
}

#[test]
fn a_mob_holding_a_chest_open_lifts_its_lid_until_it_lets_go_or_leaves() {
    use crate::events::tick::TickEvents;
    use petramond_math::math::IVec3;
    use petramond_math::world_pos::WorldPos;

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let feet = server.sessions[0].player.pos;
    let chest = IVec3::new(
        feet.x.floor() as i32 + 2,
        feet.y.floor() as i32,
        feet.z.floor() as i32,
    );
    server.world.set_block_world(
        chest.x,
        chest.y,
        chest.z,
        petramond_world::block::Block::Chest,
    );
    assert!(server.world.mobs_mut().spawn(
        crate::mob::Mob::Sheep,
        WorldPos::new(feet.x + 1.0, feet.y, feet.z),
        0.0
    ));
    let mob = server.world.mobs().instances()[0].id();

    let mut events = TickEvents::default();
    server
        .containers
        .set_mob_hold(&server.world, mob, chest, true, &mut events);
    server
        .containers
        .set_mob_hold(&server.world, mob, chest, true, &mut events);
    assert_eq!(
        server.chest_viewers(chest),
        1,
        "a mob is one viewer however often it asks"
    );
    assert_eq!(events.world.chest_changed, vec![(chest, true)]);

    server
        .containers
        .set_mob_hold(&server.world, mob, chest, false, &mut events);
    assert_eq!(server.chest_viewers(chest), 0);
    server
        .containers
        .set_mob_hold(&server.world, mob, chest, true, &mut events);

    server.world.mobs_mut().remove(mob);
    server
        .containers
        .release_absent_holders(server.world.mobs(), &mut events);
    assert_eq!(
        server.chest_viewers(chest),
        0,
        "a mob gone from the world lets go"
    );
    assert_eq!(
        events.world.chest_changed,
        vec![(chest, true), (chest, false), (chest, true), (chest, false)]
    );
}

#[test]
fn post_handlers_act_for_the_events_player_and_systems_for_nobody() {
    use crate::events::tick::TickEvents;
    use crate::events::{Attach, DamageSource, PostEventKind, Stage};
    use std::sync::{Arc, Mutex};

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let second = server.add_session_for_test(player);
    let second_id = server.sessions[second].id;

    type Seen = Arc<Mutex<Vec<(&'static str, Option<crate::player::PlayerId>, usize)>>>;
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = Arc::clone(&seen);
        server
            .mods
            .bus_mut()
            .on_post(PostEventKind::PlayerDied, 0, move |ctx, _| {
                let reachable = ctx.player_ids().len();
                seen.lock().unwrap().push(("died", ctx.actor, reachable));
            });
    }
    {
        let seen = Arc::clone(&seen);
        server
            .mods
            .systems_mut()
            .attach(Attach::Before(Stage::Mining), 0, move |ctx| {
                let reachable = ctx.player_ids().len();
                seen.lock().unwrap().push(("system", ctx.actor, reachable));
            });
    }

    let mut events = TickEvents::default();
    let health = server.sessions[second].player.health();
    server.damage_player(second, health, DamageSource::Fall, None, &mut events);
    server.game_tick_step(&mut events);

    assert_eq!(
        *seen.lock().unwrap(),
        vec![("died", Some(second_id), 2), ("system", None, 2)]
    );
}
