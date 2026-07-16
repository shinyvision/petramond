use super::super::tick::{TickEvents, TICK_DT};
use super::common::{self, filled_inventory, game, game_on_empty_chunk, hit};
use crate::block::Block;
use crate::events::{DamageSource, Outcome};
use crate::mathh::{IVec3, Vec3};
use crate::mob::{Mob, MobAttack, MobDamageFeedback};
use crate::net::protocol::{ClientToServer, PlayerAction};
use crate::player;
use crate::server::game::ATTACK_COOLDOWN_TICKS;

fn strike() -> MobAttack {
    MobAttack {
        target: crate::mob::EntityRef::Player(Default::default()),
        mob_index: 0,
        mob: Mob::Owl,
        mob_id: 1,
        origin: Vec3::new(7.0, 64.0, 8.0),
        damage: 2.0,
        knockback_dir: Vec3::new(1.0, 0.0, 0.0),
        knockback: 5.0,
    }
}

#[test]
fn a_mob_strike_damages_and_knocks_back_the_player_through_the_funnel() {
    let mut game = game();
    let mut ev = TickEvents::default();
    let health0 = game.server.sessions[0].player.health();
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    game.server.apply_mob_attacks(vec![strike()], &mut ev);

    assert_eq!(
        game.server.sessions[0].player.health(),
        health0 - 2,
        "the strike's damage lands in half-heart points"
    );
    assert!(
        game.server.sessions[0].player.vel.x > 4.0,
        "knocked back along the strike direction: {:?}",
        game.server.sessions[0].player.vel
    );
    assert!(
        game.server.sessions[0].player.vel.y > 0.0,
        "the knockback pops the player upward: {:?}",
        game.server.sessions[0].player.vel
    );
    assert!(
        !game.server.sessions[0].player.on_ground,
        "the pop reads as a launch"
    );
}

#[test]
fn engine_iframes_are_global_per_victim_for_players_and_mobs() {
    use crate::damage::{MOB_DAMAGE_IFRAME_TICKS, PLAYER_DAMAGE_IFRAME_TICKS};

    let mut game = game();
    let mut ev = TickEvents::default();
    let pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(game.server.world.mobs_mut().spawn(Mob::Sheep, pos, 0.0));
    let player_health = game.server.sessions[0].player.health();
    let mob_health = game.server.world.mobs().instances()[0].health();

    assert!(game
        .server
        .damage_player(0, 2, DamageSource::Fall, None, &mut ev));
    assert!(game.server.damage_mob_through_pipeline(
        0,
        0,
        1.0,
        DamageSource::PlayerAttack(game.server.sessions[0].id),
        Some(pos + Vec3::X),
        &mut ev,
    ));

    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.apply_mob_attacks(vec![strike()], &mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        player_health - 2,
        "mob damage is blocked by the window fall damage opened"
    );
    assert_eq!(
        game.server.sessions[0].player.vel,
        Vec3::ZERO,
        "an immune player receives no attack knockback"
    );
    assert!(!game
        .server
        .damage_mob_through_pipeline(0, 0, 1.0, DamageSource::Fall, None, &mut ev,));
    assert_eq!(
        game.server.world.mobs().instances()[0].health(),
        mob_health - 1.0
    );

    for _ in 1..MOB_DAMAGE_IFRAME_TICKS {
        game.server.game_tick_step(&mut ev);
    }
    assert!(!game
        .server
        .damage_player(0, 1, DamageSource::Mod("test"), None, &mut ev));
    assert!(!game.server.damage_mob_through_pipeline(
        0,
        0,
        1.0,
        DamageSource::Mod("test"),
        None,
        &mut ev,
    ));

    game.server.game_tick_step(&mut ev);
    assert!(!game
        .server
        .damage_player(0, 1, DamageSource::Mod("test"), None, &mut ev));
    assert!(game
        .server
        .damage_mob_through_pipeline(0, 0, 1.0, DamageSource::Fall, None, &mut ev,));

    for _ in (MOB_DAMAGE_IFRAME_TICKS + 1)..PLAYER_DAMAGE_IFRAME_TICKS {
        game.server.game_tick_step(&mut ev);
    }
    assert!(!game
        .server
        .damage_player(0, 1, DamageSource::Mod("test"), None, &mut ev));

    game.server.game_tick_step(&mut ev);
    assert!(game.server.damage_player(
        0,
        1,
        DamageSource::PlayerAttack(Default::default()),
        None,
        &mut ev,
    ));
    assert_eq!(game.server.sessions[0].player.health(), player_health - 3);
    assert_eq!(
        game.server.world.mobs().instances()[0].health(),
        mob_health - 2.0
    );
}

#[test]
fn mob_strikes_route_to_the_targeted_session_only() {
    let mut game = game();
    let other = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(30.0, 80.0, 0.0)));
    let other_id = game.server.sessions[other].id;
    let mut ev = TickEvents::default();
    let h0 = game.server.sessions[0].player.health();
    let h1 = game.server.sessions[other].player.health();

    let mut a = strike();
    a.target = crate::mob::EntityRef::Player(other_id);
    game.server.apply_mob_attacks(vec![a], &mut ev);

    assert_eq!(
        game.server.sessions[0].player.health(),
        h0,
        "the untargeted session is untouched"
    );
    assert_eq!(
        game.server.sessions[other].player.health(),
        h1 - 2,
        "the strike lands on the session its target id names"
    );
}

#[test]
fn a_cancelled_player_damage_pre_blocks_both_damage_and_knockback() {
    // Any pre-handler cancellation must suppress the strike WHOLE — no health
    // loss and no shove. That's why knockback is gated on the funnel verdict.
    let mut game = game();
    let mut ev = TickEvents::default();
    game.server
        .bus
        .on_player_damage_pre(0, |_, _| Outcome::Cancel);
    let health0 = game.server.sessions[0].player.health();
    game.server.sessions[0].player.vel = Vec3::ZERO;

    game.server.apply_mob_attacks(vec![strike()], &mut ev);

    assert_eq!(
        game.server.sessions[0].player.health(),
        health0,
        "cancel = no damage"
    );
    assert_eq!(
        game.server.sessions[0].player.vel,
        Vec3::ZERO,
        "cancel = no knockback either"
    );
}

#[test]
fn a_spectator_takes_neither_damage_nor_knockback_from_mob_strikes() {
    let mut game = game();
    let mut ev = TickEvents::default();
    game.server.sessions[0]
        .player
        .set_mode(crate::player::PlayerMode::Spectator);
    let health0 = game.server.sessions[0].player.health();

    game.server.apply_mob_attacks(vec![strike()], &mut ev);

    assert_eq!(game.server.sessions[0].player.health(), health0);
    assert_eq!(game.server.sessions[0].player.vel, Vec3::ZERO);
}

#[test]
fn a_mods_damage_player_action_routes_through_the_funnel() {
    // A mod's DamagePlayer HostCall queues a ModAction; the drain must send it
    // through Game::damage_player so handlers see it with a Mod source — and a
    // registered player_damage_pre canceller can block it.
    use crate::events::ModAction;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut game = game();
    let mut ev = TickEvents::default();
    let h0 = game.server.sessions[0].player.health();

    let seen_mod_source = Arc::new(AtomicBool::new(false));
    {
        let seen = seen_mod_source.clone();
        game.server.bus.on_player_damage_pre(0, move |_, pre| {
            if pre.source == DamageSource::Mod("testmod") {
                seen.store(true, Ordering::Relaxed);
            }
            Outcome::Continue
        });
    }
    game.server
        .bus
        .queue_mut()
        .push_action(ModAction::DamagePlayer {
            amount: 3,
            mod_id: "testmod",
        });
    game.server.apply_mod_actions(&mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        h0 - 3,
        "the queued damage applied"
    );
    assert!(
        seen_mod_source.load(Ordering::Relaxed),
        "the handler saw the distinguishable Mod source"
    );

    for _ in 0..crate::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        game.server.tick_damage_immunity();
    }

    // A priority -1 canceller runs first and blocks a later handler.
    game.server
        .bus
        .on_player_damage_pre(-1, |_, _| Outcome::Cancel);
    game.server
        .bus
        .queue_mut()
        .push_action(ModAction::DamagePlayer {
            amount: 5,
            mod_id: "testmod",
        });
    game.server.apply_mod_actions(&mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        h0 - 3,
        "a cancelling player_damage_pre blocks a mod's DamagePlayer"
    );

    // KillPlayer rides the same funnel/queue: cancelled the same way.
    game.server
        .bus
        .queue_mut()
        .push_action(crate::events::ModAction::KillPlayer { mod_id: "testmod" });
    game.server.apply_mod_actions(&mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        h0 - 3,
        "KillPlayer cancelled too"
    );
}

#[test]
fn queued_mod_actions_apply_within_a_game_tick() {
    // The wiring contract: an action sitting in the queue when a fixed tick
    // runs is applied by that tick (at its first drain point), not lost.
    use crate::events::ModAction;

    let mut game = game_on_empty_chunk();
    let mut ev = TickEvents::default();
    let h0 = game.server.sessions[0].player.health();
    game.server
        .bus
        .queue_mut()
        .push_action(ModAction::DamagePlayer {
            amount: 2,
            mod_id: "testmod",
        });
    game.server.game_tick_step(&mut ev);
    assert_eq!(game.server.sessions[0].player.health(), h0 - 2);
}

#[test]
fn closest_mob_targets_in_front_within_reach_skips_block_occluded_and_corpses() {
    let mut game = game_on_empty_chunk();
    game.cam.pos = Vec3::new(8.0, 66.0, 8.0);
    game.cam.pitch = 0.0; // level look, so the eye ray stays at constant y
    let dir = game.cam.forward();
    // An owl two metres ahead, feet dropped so the eye-level ray crosses its body.
    let mut feet = game.cam.pos + dir * 2.0;
    feet.y -= 0.35;
    assert!(game.server.world.mobs_mut().spawn(Mob::Owl, feet, 0.0));
    let id = game.server.world.mobs().instances()[0].id();

    // Targeting reads the REPLICATED rows: feed the store as a batch would.
    let rows = |game: &super::common::TestGame| -> Vec<crate::net::protocol::MobStateRow> {
        game.server
            .world
            .mobs()
            .instances()
            .iter()
            .map(|m| crate::net::protocol::MobStateRow {
                id: m.id(),
                kind_id: m.kind.0,
                pos: m.pos,
                yaw: 0.0,
                anim_time: 0.0,
                moving: false,
                idle_anim: None,
                head_yaw: 0.0,
                head_pitch: 0.0,
                hurt_timer: 0.0,
                dead: m.is_dead(),
                shorn: false,
                emitters: Vec::new(),
                anims: Vec::new(),
                ragdoll: None,
            })
            .collect()
    };
    let batch = rows(&game);
    game.replicated_mobs.apply(batch);

    assert_eq!(
        game.closest_mob(game.cam.pos, dir, player::REACH)
            .map(|(id, _)| id),
        Some(id),
        "a mob in front within reach is targeted (stable id)"
    );
    assert_eq!(
        game.closest_mob(game.cam.pos, dir, 1.0).map(|(id, _)| id),
        None,
        "a nearer block (smaller max_dist) occludes the mob"
    );
    // A corpse can't be targeted: the row replicates `dead` on the next batch.
    let cam_pos = game.cam.pos;
    assert!(game
        .server
        .world
        .mobs_mut()
        .damage_mob(
            0,
            100.0,
            Some(cam_pos),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    let batch = rows(&game);
    game.replicated_mobs.apply(batch);
    assert_eq!(
        game.closest_mob(game.cam.pos, dir, player::REACH),
        None,
        "a dead mob isn't targeted"
    );
}

#[test]
fn closest_mob_targets_the_interpolated_render_pose_not_the_future_row() {
    use crate::net::protocol::MobStateRow;

    fn row(id: u64, pos: Vec3) -> MobStateRow {
        MobStateRow {
            id,
            kind_id: Mob::Owl.0,
            pos,
            yaw: 0.0,
            anim_time: 0.0,
            moving: true,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            emitters: Vec::new(),
            anims: Vec::new(),
            ragdoll: None,
        }
    }

    let mut game = game();
    let eye = Vec3::new(8.0, 66.0, 8.0);
    let dir = Vec3::Z;
    let feet_y = eye.y - 0.35;
    let previous = eye + dir * 2.0;
    let future = eye + dir * 6.0;
    game.replicated_mobs
        .apply(vec![row(42, Vec3::new(previous.x, feet_y, previous.z))]);
    game.replicated_mobs
        .apply(vec![row(42, Vec3::new(future.x, feet_y, future.z))]);
    game.replica_clock.start();
    game.replica_clock.advance(TICK_DT * 0.5);

    assert_eq!(
        game.closest_mob(eye, dir, player::REACH).map(|(id, _)| id),
        Some(42),
        "the halfway rendered body is still in reach even though curr is not"
    );
}

#[test]
fn fist_takes_four_hits_to_kill_an_owl() {
    let mut game = game();
    let pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(game.server.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
    assert_eq!(crate::item::attack_damage(None), (1.0, 1.0));
    let from = pos + Vec3::X;
    for i in 0..3 {
        assert!(
            game.server
                .world
                .mobs_mut()
                .damage_mob(
                    0,
                    1.0,
                    Some(from),
                    true,
                    None,
                    &MobDamageFeedback::default()
                )
                .is_none(),
            "fist hit {i} isn't lethal"
        );
        for _ in 0..crate::damage::MOB_DAMAGE_IFRAME_TICKS {
            game.server.world.mobs_mut().tick_damage_immunity();
        }
    }
    assert!(
        game.server
            .world
            .mobs_mut()
            .damage_mob(
                0,
                1.0,
                Some(from),
                true,
                None,
                &MobDamageFeedback::default()
            )
            .is_some(),
        "the 4th fist hit kills"
    );
}

/// Latch an attack click at the mob at `index`, the way an
/// `Action(AttackClick)` message does — carrying the STABLE id.
fn click_attack_at(game: &mut super::common::TestGame, index: usize) {
    let id = game.server.world.mobs().instances()[index].id();
    common::aim_server_at_mob(game, index);
    game.server.sessions[0].pending_attack = true;
    game.server.sessions[0].pending_attack_mob = Some(id);
}

#[test]
fn attack_lands_next_tick_then_locks_out_for_the_cooldown() {
    let mut game = game();
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    let mut ev = TickEvents::default();

    // A click resolves on the tick (the tick after it was registered).
    click_attack_at(&mut game, 0);
    game.server.tick_attack(0, &mut ev);
    assert!(ev.player_at(0).swung_hand, "the click lands on the tick");

    // For the rest of the cooldown, a fresh click each tick lands nothing — even
    // spamming can't beat the 6-tick gate.
    for _ in 0..ATTACK_COOLDOWN_TICKS - 1 {
        ev.player(0).swung_hand = false;
        click_attack_at(&mut game, 0);
        game.server.tick_attack(0, &mut ev);
        assert!(
            !ev.player_at(0).swung_hand,
            "locked out during the cooldown"
        );
    }

    // The cooldown has now elapsed, so a pending click connects again.
    ev.player(0).swung_hand = false;
    click_attack_at(&mut game, 0);
    game.server.tick_attack(0, &mut ev);
    assert!(
        ev.player_at(0).swung_hand,
        "the cooldown elapsed, the next attack lands"
    );

    // Only two fist hits (1 dmg each) landed across all those ticks, so the 4-health
    // owl is still alive: the gate makes a spam-click instakill impossible.
    assert!(
        !game.server.world.mobs().instances()[0].is_dead(),
        "rate-limited, so the owl survives the burst"
    );
}

#[test]
fn dead_and_spectator_players_cannot_attack_mobs() {
    for spectator in [false, true] {
        let mut game = game_on_empty_chunk();
        assert!(game
            .server
            .world
            .mobs_mut()
            .spawn(Mob::Owl, Vec3::new(8.0, 200.0, 8.0), 0.0));
        click_attack_at(&mut game, 0);
        if spectator {
            game.server.sessions[0]
                .player
                .set_mode(crate::player::PlayerMode::Spectator);
        } else {
            game.server.sessions[0].player.set_health(0);
        }
        let health = game.server.world.mobs().instances()[0].health();
        let mut ev = TickEvents::default();

        game.server.tick_attack(0, &mut ev);

        assert_eq!(
            game.server.world.mobs().instances()[0].health(),
            health,
            "{} actor cannot authorize a mob hit",
            if spectator { "spectator" } else { "dead" }
        );
        assert!(
            ev.player_at(0).swung_hand,
            "a rejected claimed target still degrades to an air punch"
        );
    }
}

#[test]
fn a_newly_boarded_player_cannot_attack_their_mount_before_mirror_reconciliation() {
    let mut game = game_on_empty_chunk();
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 200.0, 8.0), 0.0));
    click_attack_at(&mut game, 0);
    let mob_id = game.server.world.mobs().instances()[0].id();
    let player_id = game.server.sessions[0].id.0;
    let health = game.server.world.mobs().instances()[0].health();

    // Placement runs before Attack. A successful board therefore updates the
    // authoritative registry while the session mirror remains stale until
    // the later Riding pass.
    assert!(game.server.world.riding_mut().mount(player_id, mob_id, 0));
    assert!(game.server.sessions[0].mount.is_none());
    let mut events = TickEvents::default();

    game.server.tick_attack(0, &mut events);

    assert_eq!(game.server.world.mobs().instances()[0].health(), health);
    assert!(
        events.player_at(0).swung_hand,
        "the rejected own-mount claim degrades to an air punch"
    );
}

#[test]
fn a_forged_mob_id_cannot_redirect_an_attack_past_the_nearest_body() {
    let mut game = game_on_empty_chunk();
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 200.0, 8.0), 0.0));
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 200.0, 9.0), 0.0));
    common::aim_server_at_mob(&mut game, 0);
    let forged = game.server.world.mobs().instances()[1].id();
    let health: Vec<_> = game
        .server
        .world
        .mobs()
        .instances()
        .iter()
        .map(|mob| mob.health())
        .collect();
    game.server.sessions[0].pending_attack = true;
    game.server.sessions[0].pending_attack_mob = Some(forged);
    let mut ev = TickEvents::default();

    game.server.tick_attack(0, &mut ev);

    let after: Vec<_> = game
        .server
        .world
        .mobs()
        .instances()
        .iter()
        .map(|mob| mob.health())
        .collect();
    assert_eq!(after, health, "authority never redirects the claimed id");
    assert!(ev.player_at(0).swung_hand, "the rejected click air-punches");
}

#[test]
fn opening_a_screen_drops_a_latched_action_so_it_cant_fire_behind_the_menu() {
    let mut game = game();
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    let mob_id = game.server.world.mobs().instances()[0].id();

    // A click message latches while playing...
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::AttackClick {
            mob: Some(mob_id),
            player: None,
        }),
    );
    assert!(
        game.server.sessions[0].pending_attack,
        "the click latched while playing"
    );

    // ...then a screen takes input focus before any tick ran (the next frame's
    // PlayerUpdate reports gameplay=false). The latched press is dropped, so
    // the tick that still runs behind the menu lands no attack.
    let update = common::player_update(&game, false);
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(update));
    assert!(
        !game.server.sessions[0].pending_attack,
        "opening a screen drops the latched press"
    );
    assert!(
        game.server.sessions[0].pending_attack_mob.is_none(),
        "the click's mob target is dropped with it"
    );
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);
    assert!(
        !ev.player_at(0).swung_hand,
        "no attack fires behind the open menu"
    );
}

#[test]
fn a_killed_mob_ragdolls_then_despawns() {
    let mut game = game_on_empty_chunk();
    let pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(game.server.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
    assert!(game
        .server
        .world
        .mobs_mut()
        .damage_mob(
            0,
            100.0,
            Some(pos + Vec3::X),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    assert_eq!(
        game.server.world.mobs().len(),
        1,
        "the corpse is present while ragdolling"
    );
    let player_pos = game.server.sessions[0].player.body_center();
    let player_body = game.server.sessions[0].player.body();
    // 1.5 s ragdoll lifetime at 20 TPS = 30 ticks; run extra for margin.
    for _ in 0..50 {
        game.server.world.tick_mobs(
            TICK_DT,
            &[crate::mob::PlayerAnchor {
                id: Default::default(),
                pos: player_pos,
                body: Some(player_body),
                sneaking: false,
            }],
        );
    }
    assert_eq!(
        game.server.world.mobs().len(),
        0,
        "the corpse despawns once the ragdoll finishes"
    );
}

#[test]
fn mobs_take_player_rule_fall_damage_when_they_land() {
    let mut game = game();
    game.server.world.clear_world();
    let mut chunk = crate::chunk::Chunk::new(0, 0);
    for z in 0..crate::chunk::CHUNK_SZ {
        for x in 0..crate::chunk::CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Grass);
        }
    }
    game.server
        .world
        .insert_chunk_for_test(crate::chunk::ChunkPos::new(0, 0), chunk);

    let spawn = Vec3::new(8.5, 70.0, 8.5);
    assert!(game.server.world.mobs_mut().spawn(Mob::Owl, spawn, 0.0));
    let health0 = game.server.world.mobs().instances()[0].health();
    let player = game.server.sessions[0].player.body_center();
    let body = game.server.sessions[0].player.body();
    let anchors = [crate::mob::PlayerAnchor {
        id: game.server.sessions[0].id,
        pos: player,
        body: Some(body),
        sneaking: false,
    }];

    let mut feed = TickEvents::default();
    let mut landed = false;
    for _ in 0..80 {
        let mob_events = game.server.world.tick_mobs(TICK_DT, &anchors);
        landed |= !mob_events.falls.is_empty();
        game.server
            .apply_mob_fall_damage(mob_events.falls, &mut feed);
        if landed {
            break;
        }
    }

    assert!(landed, "the mob landed and reported a fall");
    let mob = &game.server.world.mobs().instances()[0];
    let expected = crate::server::health::fall_damage_health(spawn.y - 64.0) as f32;
    assert_eq!(expected, 3.0, "fixture is a six-block fall");
    assert_eq!(mob.health(), health0 - expected);
    assert!(!mob.is_dead(), "the owl survives this fall at one health");
}

#[test]
fn killing_owls_drops_loot_into_the_world() {
    let mut game = game_on_empty_chunk();
    let pos = Vec3::new(8.0, 64.0, 8.0);
    // Over many kills the owl table (50% sticks / 25% coal) virtually always yields
    // something — this proves the death→loot path is wired, without pinning the
    // (freely-editable) table contents.
    for _ in 0..40 {
        assert!(game.server.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
        let idx = game.server.world.mobs().len() - 1;
        if let Some(death) = game.server.world.mobs_mut().damage_mob(
            idx,
            100.0,
            Some(pos + Vec3::X),
            true,
            None,
            &MobDamageFeedback::default(),
        ) {
            game.server.spawn_mob_loot(death);
        }
    }
    assert!(
        !game.server.world.item_entities().is_empty(),
        "killing owls drops loot via the loot table"
    );
}

#[test]
fn a_mob_pushes_the_player_per_frame() {
    // The player is shoved out of an overlapping mob every frame (not on the tick),
    // so the drift is smooth. An owl just east of the player pushes it west.
    // The push acts on the CLIENT's predicted player against the REPLICATED
    // mob rows (the shove reaches the server in the next PlayerUpdate).
    let mut game = game();
    game.player.pos = Vec3::new(8.0, 64.0, 8.0);
    game.replicated_mobs
        .apply(vec![crate::net::protocol::MobStateRow {
            id: 1,
            kind_id: Mob::Owl.0,
            pos: Vec3::new(8.2, 64.0, 8.0),
            yaw: 0.0,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            emitters: Vec::new(),
            anims: Vec::new(),
            ragdoll: None,
        }]);
    let x0 = game.player.pos.x;
    for _ in 0..30 {
        game.apply_entity_push(1.0 / 60.0);
    }
    assert!(
        game.player.pos.x < x0 - 0.05,
        "the owl pushed the player -X, away from it: {x0} -> {}",
        game.player.pos.x
    );
}

#[test]
fn a_remote_player_pushes_the_local_player_per_frame() {
    // Remote players jostle like mobs: an overlapping remote body shoves the
    // LOCAL predicted player out, per frame, through the same separation rule.
    // The remote's own half runs on its own client — each client only ever
    // shoves itself. Hidden bodies (spectators/the dead) and sleepers don't
    // push: nothing should nudge the player off a bedside vigil, and nothing
    // is there to touch when the body isn't rendered.
    use crate::net::protocol::PlayerStateRow;
    use crate::server::player::PlayerId;
    use std::collections::HashMap;

    fn remote_row(pos: Vec3, visible: bool, sleeping: bool) -> PlayerStateRow {
        PlayerStateRow {
            id: PlayerId(1),
            pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            sneaking: false,
            sleeping,
            sleep_yaw: None,
            alive: visible,
            visible,
            held_item: None,
            mining: None,
            eating: false,
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    let mut game = game();
    let own_id = game.game.self_id;
    let roster = HashMap::new();
    let start = Vec3::new(8.0, 64.0, 8.0);
    let overlap = Vec3::new(8.2, 64.0, 8.0); // just east, footprints overlapping

    let run = |game: &mut common::TestGame, row: PlayerStateRow| {
        game.player.pos = start;
        game.game.remote_players.apply(&[row], &[], own_id, &roster);
        for _ in 0..30 {
            game.apply_entity_push(1.0 / 60.0);
        }
        game.player.pos.x - start.x
    };

    let moved = run(&mut game, remote_row(overlap, true, false));
    assert!(
        moved < -0.05,
        "the remote body pushed the player -X, away from it: {moved}"
    );

    let hidden = run(&mut game, remote_row(overlap, false, false));
    assert_eq!(hidden, 0.0, "a hidden (spectator/dead) remote doesn't push");

    let asleep = run(&mut game, remote_row(overlap, true, true));
    assert_eq!(asleep, 0.0, "a sleeping remote doesn't push");
}

#[test]
fn cannot_place_a_solid_block_inside_a_mob() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = filled_inventory(); // a stack of Dirt
    game.server.sessions[0].player.inventory.set_active(0);
    // Park the player far off so only the mob can block placement here.
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);

    // An owl standing in cell (8, 200, 8), high up and clear of the player.
    assert!(game
        .server
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.5, 200.0, 8.5), 0.0));

    // Aiming a Dirt block into the owl's cell does nothing: no block lands and the
    // held stack isn't consumed.
    let before = game.server.sessions[0]
        .player
        .inventory
        .selected()
        .unwrap()
        .count;
    game.server.sessions[0].look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y)); // p = (8, 200, 8)
    assert!(
        !game.server.try_place_for_test(),
        "a solid block can't be placed inside the owl"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(8, 200, 8)),
        Block::Air,
        "nothing was placed"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        before,
        "the held item wasn't consumed"
    );

    // A cell clear of the owl (and the player) places as usual.
    game.server.sessions[0].look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y)); // p = (0, 200, 0)
    assert!(
        game.server.try_place_for_test(),
        "an empty cell places normally"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(0, 200, 0)),
        Block::Dirt
    );
}

#[test]
fn cannot_place_a_solid_block_inside_another_player() {
    let mut game = game_on_empty_chunk();
    game.server.sessions[0].player.inventory = filled_inventory(); // a stack of Dirt
    game.server.sessions[0].player.inventory.set_active(0);
    // Park the placer far off so only the other session can block placement here.
    game.server.sessions[0].player.pos = Vec3::new(100.0, 64.0, 100.0);

    let other = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(8.5, 200.0, 8.5)));

    let before = game.server.sessions[0]
        .player
        .inventory
        .selected()
        .unwrap()
        .count;
    game.server.sessions[0].look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y)); // p = (8, 200, 8)
    assert!(
        !game.server.try_place_for_test(),
        "a solid block can't be placed inside another live player"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(8, 200, 8)),
        Block::Air,
        "nothing was placed"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        before,
        "the held item wasn't consumed"
    );

    game.server.sessions[other]
        .player
        .set_mode(crate::player::PlayerMode::Spectator);
    game.server.sessions[0].look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y));
    assert!(
        game.server.try_place_for_test(),
        "a spectator has no placement-blocking body"
    );
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(8, 200, 8)),
        Block::Dirt
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .unwrap()
            .count,
        before - 1,
        "successful placement consumes one item"
    );
}

/// Latch a PvP attack click at `target`, the way an `Action(AttackClick)`
/// message does (mob and player are mutually exclusive on a click).
fn click_attack_player(
    game: &mut super::common::TestGame,
    target: crate::server::player::PlayerId,
) {
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::AttackClick {
            mob: None,
            player: Some(target.0),
        }),
    );
}

/// Two sessions in reach; a fist guarantees the deterministic (1.0, 1.0)
/// damage roll.
fn pvp_pair(game: &mut super::common::TestGame) -> usize {
    game.server.sessions[0].player.pos = Vec3::new(0.5, 64.0, 0.5);
    game.server.sessions[0].player.inventory = crate::inventory::Inventory::new();
    let t = game
        .server
        .add_session_for_test(crate::player::Player::new(Vec3::new(2.5, 64.0, 0.5)));
    game.server.sessions[t].player.vel = Vec3::ZERO;
    t
}

#[test]
fn a_pvp_attack_damages_the_target_through_the_funnel_with_knockback_and_cooldown() {
    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.server.sessions[t].id;
    let h0 = game.server.sessions[t].player.health();
    let attacker_h0 = game.server.sessions[0].player.health();

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);

    assert!(ev.player_at(0).swung_hand, "the hit swings the hand");
    assert_eq!(
        game.server.sessions[0].attack_cooldown, ATTACK_COOLDOWN_TICKS,
        "the swing arms the cooldown, exactly like a mob hit"
    );
    assert_eq!(
        game.server.sessions[t].player.health(),
        h0 - 1,
        "a fist hit costs the target one half-heart"
    );
    assert!(
        ev.player_at(t).player_damaged,
        "the victim's damaged one-shot fires (hurt sound/shake/hurt_recent)"
    );
    assert_eq!(
        game.server.sessions[0].player.health(),
        attacker_h0,
        "only the target is damaged"
    );
    let vel = game.server.sessions[t].player.vel;
    assert!(
        vel.x > 0.0,
        "knocked horizontally away from the attacker: {vel:?}"
    );
    assert!(vel.y > 0.0, "with the mob-strike upward pop: {vel:?}");
}

#[test]
fn a_pvp_attack_out_of_reach_lands_no_damage() {
    let mut game = game();
    let t = pvp_pair(&mut game);
    game.server.sessions[t].player.pos = Vec3::new(20.5, 64.0, 0.5); // beyond REACH + 1
    let h0 = game.server.sessions[t].player.health();
    let target_id = game.server.sessions[t].id;

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);

    assert_eq!(game.server.sessions[t].player.health(), h0, "no damage");
    assert_eq!(
        game.server.sessions[t].player.vel,
        Vec3::ZERO,
        "no knockback"
    );
}

#[test]
fn spectators_neither_attack_nor_take_pvp_hits() {
    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.server.sessions[t].id;

    // A spectator TARGET can't be hit.
    game.server.sessions[t]
        .player
        .set_mode(crate::player::PlayerMode::Spectator);
    let h0 = game.server.sessions[t].player.health();
    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);
    assert_eq!(game.server.sessions[t].player.health(), h0);
    assert_eq!(game.server.sessions[t].player.vel, Vec3::ZERO);

    // A spectator ATTACKER can't hit.
    game.server.sessions[t]
        .player
        .set_mode(crate::player::PlayerMode::Survival);
    game.server.sessions[0]
        .player
        .set_mode(crate::player::PlayerMode::Spectator);
    game.server.sessions[0].attack_cooldown = 0;
    let h0 = game.server.sessions[t].player.health();
    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);
    assert_eq!(game.server.sessions[t].player.health(), h0);
}

#[test]
fn a_cancelled_player_damage_pre_suppresses_pvp_damage_and_knockback() {
    use std::sync::{Arc, Mutex};

    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.server.sessions[t].id;
    let attacker_id = game.server.sessions[0].id;
    let seen = Arc::new(Mutex::new(None));
    {
        let seen = seen.clone();
        game.server.bus.on_player_damage_pre(0, move |_, pre| {
            *seen.lock().unwrap() = Some(pre.source);
            Outcome::Cancel
        });
    }
    let h0 = game.server.sessions[t].player.health();

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);

    assert_eq!(
        game.server.sessions[t].player.health(),
        h0,
        "cancel = no damage"
    );
    assert_eq!(
        game.server.sessions[t].player.vel,
        Vec3::ZERO,
        "cancel = no knockback either"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        Some(DamageSource::PlayerAttack(attacker_id)),
        "the funnel saw the PvP source with the attacker's id"
    );
}

/// The knockback is tick-side VELOCITY-only (position follows client-side),
/// so the victim's transform-drift check must catch a vel change and ship the
/// `SelfState::transform` echo — otherwise the victim's own physics never
/// learns the new velocity.
#[test]
fn pvp_knockback_ships_the_victims_vel_echo() {
    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.server.sessions[t].id;
    // What the victim's client last claimed: its exact pre-hit transform.
    let reported = {
        let p = &game.server.sessions[t].player;
        crate::net::protocol::SelfTransform {
            pos: p.pos,
            vel: p.vel,
            yaw: p.yaw,
            pitch: p.pitch,
            on_ground: p.on_ground,
        }
    };
    game.server.sessions[t].last_reported_transform = Some(reported);

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);

    let state = game.server.build_self_state(t);
    let echo = state
        .transform
        .expect("a vel-only knockback still ships the transform correction");
    assert_eq!(echo.pos, reported.pos, "the tick moved no position");
    assert_ne!(
        echo.vel, reported.vel,
        "the echo carries the knocked velocity"
    );
    assert_eq!(
        echo.vel, game.server.sessions[t].player.vel,
        "the echoed velocity is the session's post-knockback one"
    );
}

/// Client-side PvP targeting: a visible, alive remote body under the
/// crosshair is targeted (nearest wins vs mobs; at most one target kind is
/// set); dead/invisible remotes are ignored.
#[test]
fn refresh_target_picks_remote_players_competing_with_mobs() {
    use crate::net::protocol::PlayerStateRow;
    use crate::server::player::PlayerId;
    use std::collections::HashMap;

    fn remote_row(id: u8, pos: Vec3, visible: bool) -> PlayerStateRow {
        PlayerStateRow {
            id: PlayerId(id),
            pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            sneaking: false,
            sleeping: false,
            sleep_yaw: None,
            alive: visible,
            visible,
            held_item: None,
            mining: None,
            eating: false,
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    let mut game = game_on_empty_chunk();
    game.cam.pos = Vec3::new(8.0, 66.0, 8.0);
    game.cam.pitch = 0.0;
    let dir = game.cam.forward();
    let own_id = game.game.self_id;
    let roster = HashMap::new();

    // A remote body two metres ahead, feet dropped so the level ray crosses it.
    let mut feet = game.cam.pos + dir * 2.0;
    feet.y -= 1.0;
    game.game
        .remote_players
        .apply(&[remote_row(1, feet, true)], &[], own_id, &roster);
    game.refresh_target();
    assert_eq!(game.targeted_player, Some(1), "the remote body is targeted");
    assert!(game.targeted_mob.is_none(), "at most one target kind");
    assert!(
        game.look.is_none(),
        "an entity target clears the block look"
    );

    // A mob NEARER than the remote wins the distance competition.
    let mut mob_feet = game.cam.pos + dir * 1.2;
    mob_feet.y -= 0.35;
    game.game
        .replicated_mobs
        .apply(vec![crate::net::protocol::MobStateRow {
            id: 42,
            kind_id: Mob::Owl.0,
            pos: mob_feet,
            yaw: 0.0,
            anim_time: 0.0,
            moving: false,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            emitters: Vec::new(),
            anims: Vec::new(),
            ragdoll: None,
        }]);
    game.refresh_target();
    assert_eq!(game.targeted_mob, Some(42), "the nearer mob wins");
    assert!(game.targeted_player.is_none());

    // A hidden (dead/spectator) remote is never targeted.
    game.game.replicated_mobs.apply(Vec::new());
    game.game
        .remote_players
        .apply(&[remote_row(1, feet, false)], &[], own_id, &roster);
    game.refresh_target();
    assert!(
        game.targeted_player.is_none(),
        "hidden bodies are untargetable"
    );
}
