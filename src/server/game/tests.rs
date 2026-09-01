use crate::net::protocol::ServerToClient;
use crate::server::chat::ChatTargets;

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
    let player = crate::server::session_build::spawn_player(server.world.seed);
    let remote_s = server.add_session_for_test(player);
    let remote_id = server.sessions[remote_s].id;

    server.enqueue_authored_chat("only-remote", ChatTargets::Players(vec![remote_id]));
    server.enqueue_authored_chat("everyone", ChatTargets::All);

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

/// The body-claim chain a mod's tick system drives, end to end through the
/// server: a claim published for one session reaches that session's movement,
/// releasing it puts the body back, and neither touches anybody else.
///
/// The engine owes the CHAIN — roster publish, the addressed write, the fold,
/// the speed the movement code reads. What a pack decides to claim, and when,
/// is the pack's business and is tested there.
#[test]
fn a_published_body_claim_reaches_the_addressed_sessions_movement() {
    use crate::player::MOVE_SCALE_DEFAULT;

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let other = crate::server::session_build::spawn_player(server.world.seed);
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

    // The claim is what the movement integrator reads, not a parallel flag.
    let walk = crate::player::Input {
        wishdir: petramond_math::math::Vec3::new(1.0, 0.0, 0.0),
        jump: false,
        sprint: false,
        sneak: false,
    };
    let full = server.sessions[0].player.wish_speed(walk);
    assert_eq!(server.sessions[s].player.wish_speed(walk), full * 0.25);

    // ...and releasing puts the body back, which is what makes an unloaded or
    // silent mod self-healing rather than a permanent slow.
    server.sessions[s].player.claims.clear();
    assert_eq!(server.sessions[s].player.wish_speed(walk), full);
}

/// A raised guard is a mod's business, but the ACTOR CONTEXT it reads is the
/// engine's: a `player_damage_pre` handler must see the victim's own live
/// intents, and a cancel must stop the hit AND the knockback that rides on it.
///
/// The pre-damage dispatch's own regression test (`server::health`) pins the
/// naming; this pins what a handler can DO with it.
#[test]
fn a_cancelled_pre_damage_applies_neither_damage_nor_its_knockback() {
    use crate::events::{tick::TickEvents, DamageSource, Outcome};

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    server.sessions[0].intent_gameplay = true;
    server.sessions[0].intent_use_held = true;
    server.publish_player_inputs();

    // The handler cancels on a session-side intent — the class of predicate
    // that silently answered its default before the dispatch named its victim.
    server.bus.on_player_damage_pre(0, move |ctx, _ev| {
        let holding_use = ctx
            .acting_player_id()
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

    // Release the intent and the same strike lands — non-vacuous.
    server.sessions[0].intent_use_held = false;
    assert!(hit(&mut server), "an unguarded strike must land");
    assert_eq!(server.sessions[0].player.health(), 16);
}

/// A mod cue (`EmitEventTo`) rides the recipient's own batch, and ONLY that
/// recipient's: it is the one lane a pack can address a single player's client
/// through, so misdelivering it would present another player's shield taking a
/// hit on your screen — with nothing anywhere to say so.
///
/// It is also the one NON-lossy field of `SelfEvents`. Every other one-shot
/// there is a latch the newest write wins; a pack's cues are a queue, and two
/// in one replication window must both arrive, in order.
#[test]
fn a_mod_cue_reaches_only_the_session_it_names_and_never_coalesces() {
    use crate::events::{tick::TickEvents, ClientEvent};

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.seed);
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
            .build_tick_update(s, &events, &[], &[], &[], &[], &shared)
            .self_events
            .client_events
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

/// A mod-denied body cannot punch or mine, and the engine enforces it at BOTH
/// gates — the button and the timer.
///
/// It is the chain the engine owes: the claim lands on the body, the per-tick
/// stages read it, and a denied press leaves no trace. What a pack denies, and
/// when, is the pack's business and is tested there.
///
/// The two failure modes worth pinning: a denied attack press that QUEUES
/// instead of being spent (releasing the claim would then fire a stored punch),
/// and a denied mine that merely pauses (the crack would resume mid-block on a
/// cell the player has long stopped looking at).
#[test]
fn a_denied_body_cannot_swing_or_run_its_mining_timer() {
    use crate::events::tick::TickEvents;
    use crate::player::DeniedActions;
    use mod_api::BodyAction::{Attack, Mine};
    use petramond_math::math::IVec3;

    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    server.sessions[0].intent_gameplay = true;

    // A solid cell right under the player's feet, targeted and being mined.
    let feet = server.sessions[0].player.pos;
    let cell = IVec3::new(
        feet.x.floor() as i32,
        feet.y.floor() as i32 - 1,
        feet.z.floor() as i32,
    );
    server
        .world
        .set_block_world(cell.x, cell.y, cell.z, petramond_world::block::Block::Stone);
    server.sessions[0].look = Some(crate::net::protocol::TargetRef {
        block: cell,
        normal: IVec3::new(0, 1, 0),
    });
    server.sessions[0].intent_break_held = true;

    let mut events = TickEvents::default();
    server.tick_mining(0, &mut events);
    assert_eq!(
        server.sessions[0].mining.overlay().map(|(p, _)| p),
        Some(cell),
        "an unclaimed body mines what it is looking at"
    );

    server.sessions[0]
        .player
        .claims
        .set_denied_actions("combat", DeniedActions::of([Attack, Mine]));
    server.tick_mining(0, &mut events);
    assert!(
        server.sessions[0].mining.overlay().is_none(),
        "a denied mine RESETS the timer, it does not pause it"
    );

    // The attack half. Look at NOTHING first: a click on a block is mining,
    // and never swings whatever the claim says — asserting "no swing" with a
    // cell under the crosshair passes for the wrong reason.
    server.sessions[0].look = None;
    server.sessions[0].pending_attack = true;
    server.tick_attack(0, &mut events);
    assert!(!events.player_at(0).swung_hand, "no swing while denied");
    assert_eq!(
        server.sessions[0].attack_cooldown, 0,
        "a denied swing arms no cooldown — it did not happen"
    );

    server.sessions[0].player.claims.clear();
    server.tick_attack(0, &mut events);
    assert!(
        !events.player_at(0).swung_hand,
        "and the denied press was SPENT, not stored for the release"
    );

    // Non-vacuous: the same press on a released body swings.
    server.sessions[0].pending_attack = true;
    server.tick_attack(0, &mut events);
    assert!(events.player_at(0).swung_hand);
}
