use super::common::{self, filled_inventory, game, game_on_empty_chunk, hit};
use petramond::events::tick::{TickEvents, TICK_DT};
use petramond::events::{DamageSource, Outcome};
use petramond::mob::{Mob, MobAttack, MobDamageFeedback};
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::player;
use petramond::rules::combat::ATTACK_COOLDOWN_TICKS;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

fn strike() -> MobAttack {
    MobAttack {
        target: petramond::mob::EntityRef::Player(Default::default()),
        mob: Mob::Owl,
        mob_id: 1,
        origin: WorldPos::new(7.0, 64.0, 8.0),
        damage: 2.0,
        knockback_dir: Vec3::new(1.0, 0.0, 0.0),
        knockback: 5.0,
    }
}

#[test]
fn a_mob_strike_damages_and_knocks_back_the_player_through_the_funnel() {
    let mut game = game();
    let mut ev = TickEvents::default();
    let health0 = game.server_player().health();
    game.server_player_mut().vel = Vec3::ZERO;
    game.server_player_mut().on_ground = true;

    game.sim_mut().apply_mob_attacks(vec![strike()], &mut ev);

    assert_eq!(
        game.server_player().health(),
        health0 - 2,
        "the strike's damage lands in half-heart points"
    );
    assert!(
        game.server_player().vel.x > 4.0,
        "knocked back along the strike direction: {:?}",
        game.server_player().vel
    );
    assert!(
        game.server_player().vel.y > 0.0,
        "the knockback pops the player upward: {:?}",
        game.server_player().vel
    );
    assert!(!game.server_player().on_ground, "the pop reads as a launch");
}

/// The mob i-frame window is a pipeline COMPONENT (`petramond:immunity`): a
/// request whose composed pipeline omits it (burn-style DoT) is neither
/// blocked by an active window nor grants one, while the default pipeline
/// both grants and is blocked.
#[test]
fn immunity_is_a_composable_pipeline_component() {
    use petramond::mob::{MobDamageFeedback, MobDamageFeedbackComponent};

    let dot = MobDamageFeedback {
        components: vec![
            MobDamageFeedbackComponent::DecreaseHealth,
            MobDamageFeedbackComponent::Flash { duration: 0.3 },
        ],
    };
    let mut game = game();
    let mut ev = TickEvents::default();
    let pos = WorldPos::new(8.0, 64.0, 8.0);
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Sheep, pos, 0.0));
    let health = game.server_world().mobs().instances()[0].health();
    let attacker = game.session().id();
    let mob_id = game.server_world().mobs().instances()[0].id();

    // A default-pipeline hit opens the window…
    assert!(game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::PlayerAttack(attacker),
        Some(pos + Vec3::X),
        None,
        &mut ev,
    ));
    // …which blocks a second default hit, but NOT the DoT pipeline: it
    // applies inside the window and grants nothing.
    assert!(!game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::Mod("test"),
        None,
        None,
        &mut ev,
    ));
    for _ in 0..3 {
        assert!(game.sim_mut().damage_mob_through_pipeline(
            mob_id,
            1.0,
            DamageSource::Mod("test"),
            None,
            Some(dot.clone()),
            &mut ev,
        ));
    }
    assert_eq!(
        game.server_world().mobs().instances()[0].health(),
        health - 4.0,
        "one default hit + three DoT ticks all landed"
    );
    // The DoT hits granted no window: once the original expires, the next
    // default hit lands on schedule.
    for _ in 1..petramond_world::damage::MOB_DAMAGE_IFRAME_TICKS {
        game.sim_mut().game_tick_step(&mut ev);
    }
    game.sim_mut().game_tick_step(&mut ev);
    assert!(game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::Fall,
        None,
        None,
        &mut ev,
    ));
}

#[test]
fn engine_iframes_are_global_per_victim_for_players_and_mobs() {
    use petramond_world::damage::{MOB_DAMAGE_IFRAME_TICKS, PLAYER_DAMAGE_IFRAME_TICKS};

    let mut game = game();
    let mut ev = TickEvents::default();
    let pos = WorldPos::new(8.0, 64.0, 8.0);
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Sheep, pos, 0.0));
    let player_health = game.server_player().health();
    let mob_health = game.server_world().mobs().instances()[0].health();
    let attacker = game.session().id();
    let mob_id = game.server_world().mobs().instances()[0].id();

    assert!(game
        .sim_mut()
        .damage_player(0, 2, DamageSource::Fall, None, &mut ev));
    assert!(game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::PlayerAttack(attacker),
        Some(pos + Vec3::X),
        None,
        &mut ev,
    ));

    game.server_player_mut().vel = Vec3::ZERO;
    game.sim_mut().apply_mob_attacks(vec![strike()], &mut ev);
    assert_eq!(
        game.server_player().health(),
        player_health - 2,
        "mob damage is blocked by the window fall damage opened"
    );
    assert_eq!(
        game.server_player().vel,
        Vec3::ZERO,
        "an immune player receives no attack knockback"
    );
    assert!(!game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::Fall,
        None,
        None,
        &mut ev,
    ));
    assert_eq!(
        game.server_world().mobs().instances()[0].health(),
        mob_health - 1.0
    );

    for _ in 1..MOB_DAMAGE_IFRAME_TICKS {
        game.sim_mut().game_tick_step(&mut ev);
    }
    assert!(!game
        .sim_mut()
        .damage_player(0, 1, DamageSource::Mod("test"), None, &mut ev));
    assert!(!game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::Mod("test"),
        None,
        None,
        &mut ev,
    ));

    game.sim_mut().game_tick_step(&mut ev);
    assert!(!game
        .sim_mut()
        .damage_player(0, 1, DamageSource::Mod("test"), None, &mut ev));
    assert!(game.sim_mut().damage_mob_through_pipeline(
        mob_id,
        1.0,
        DamageSource::Fall,
        None,
        None,
        &mut ev,
    ));

    for _ in (MOB_DAMAGE_IFRAME_TICKS + 1)..PLAYER_DAMAGE_IFRAME_TICKS {
        game.sim_mut().game_tick_step(&mut ev);
    }
    assert!(!game
        .sim_mut()
        .damage_player(0, 1, DamageSource::Mod("test"), None, &mut ev));

    game.sim_mut().game_tick_step(&mut ev);
    assert!(game.sim_mut().damage_player(
        0,
        1,
        DamageSource::PlayerAttack(Default::default()),
        None,
        &mut ev,
    ));
    assert_eq!(game.server_player().health(), player_health - 3);
    assert_eq!(
        game.server_world().mobs().instances()[0].health(),
        mob_health - 2.0
    );
}

#[test]
fn mob_strikes_route_to_the_targeted_session_only() {
    let mut game = game();
    let other = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(WorldPos::new(
            30.0, 80.0, 0.0,
        )));
    let other_id = game.session_at(other).id();
    let mut ev = TickEvents::default();
    let h0 = game.server_player().health();
    let h1 = game.session_at(other).player().health();

    let mut a = strike();
    a.target = petramond::mob::EntityRef::Player(other_id);
    game.sim_mut().apply_mob_attacks(vec![a], &mut ev);

    assert_eq!(
        game.server_player().health(),
        h0,
        "the untargeted session is untouched"
    );
    assert_eq!(
        game.session_at(other).player().health(),
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
    game.sim_mut()
        .bus_mut()
        .on_player_damage_pre(0, |_, _| Outcome::Cancel);
    let health0 = game.server_player().health();
    game.server_player_mut().vel = Vec3::ZERO;

    game.sim_mut().apply_mob_attacks(vec![strike()], &mut ev);

    assert_eq!(game.server_player().health(), health0, "cancel = no damage");
    assert_eq!(
        game.server_player().vel,
        Vec3::ZERO,
        "cancel = no knockback either"
    );
}

#[test]
fn a_spectator_takes_neither_damage_nor_knockback_from_mob_strikes() {
    let mut game = game();
    let mut ev = TickEvents::default();
    game.server_player_mut()
        .set_mode(petramond::player::PlayerMode::Spectator);
    let health0 = game.server_player().health();

    game.sim_mut().apply_mob_attacks(vec![strike()], &mut ev);

    assert_eq!(game.server_player().health(), health0);
    assert_eq!(game.server_player().vel, Vec3::ZERO);
}

#[test]
fn a_mods_damage_player_action_routes_through_the_funnel() {
    // A mod's DamagePlayer HostCall queues a DeferredAction; the drain must send it
    // through Game::damage_player so handlers see it with a Mod source — and a
    // registered player_damage_pre canceller can block it.
    use petramond::events::DeferredAction;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut game = game();
    let mut ev = TickEvents::default();
    let h0 = game.server_player().health();

    let seen_mod_source = Arc::new(AtomicBool::new(false));
    {
        let seen = seen_mod_source.clone();
        game.sim_mut()
            .bus_mut()
            .on_player_damage_pre(0, move |_, pre| {
                if pre.source == DamageSource::Mod("testmod") {
                    seen.store(true, Ordering::Relaxed);
                }
                Outcome::Continue
            });
    }
    let player = game.session().id();
    game.sim_mut()
        .bus_mut()
        .queue_mut()
        .push_action(DeferredAction::DamagePlayer {
            player,
            amount: 3,
            source: DamageSource::Mod("testmod"),
            origin: None,
        });
    game.sim_mut().apply_deferred_actions(&mut ev);
    assert_eq!(
        game.server_player().health(),
        h0 - 3,
        "the queued damage applied"
    );
    assert!(
        seen_mod_source.load(Ordering::Relaxed),
        "the handler saw the distinguishable Mod source"
    );

    for _ in 0..petramond_world::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        game.sim_mut().tick_damage_immunity();
    }

    // A priority -1 canceller runs first and blocks a later handler.
    game.sim_mut()
        .bus_mut()
        .on_player_damage_pre(-1, |_, _| Outcome::Cancel);
    let player = game.session().id();
    game.sim_mut()
        .bus_mut()
        .queue_mut()
        .push_action(DeferredAction::DamagePlayer {
            player,
            amount: 5,
            source: DamageSource::Mod("testmod"),
            origin: None,
        });
    game.sim_mut().apply_deferred_actions(&mut ev);
    assert_eq!(
        game.server_player().health(),
        h0 - 3,
        "a cancelling player_damage_pre blocks a mod's DamagePlayer"
    );
}

#[test]
fn queued_mod_actions_apply_within_a_game_tick() {
    // The wiring contract: an action sitting in the queue when a fixed tick
    // runs is applied by that tick (at its first drain point), not lost.
    use petramond::events::DeferredAction;

    let mut game = game_on_empty_chunk();
    let mut ev = TickEvents::default();
    let h0 = game.server_player().health();
    let player = game.session().id();
    game.sim_mut()
        .bus_mut()
        .queue_mut()
        .push_action(DeferredAction::DamagePlayer {
            player,
            amount: 2,
            source: DamageSource::Mod("testmod"),
            origin: None,
        });
    game.sim_mut().game_tick_step(&mut ev);
    assert_eq!(game.server_player().health(), h0 - 2);
}

#[test]
fn closest_mob_targets_in_front_within_reach_skips_block_occluded_and_corpses() {
    let mut game = game_on_empty_chunk();
    game.local.cam.pos = WorldPos::new(8.0, 66.0, 8.0);
    game.local.cam.pitch = 0.0; // level look, so the eye ray stays at constant y
    let dir = game.local.cam.forward();
    // An owl two metres ahead, feet dropped so the eye-level ray crosses its body.
    let mut feet = game.local.cam.pos + dir * 2.0;
    feet.y -= 0.35;
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, feet, 0.0));
    let id = game.server_world().mobs().instances()[0].id();

    // Targeting reads the REPLICATED rows: feed the store as a batch would.
    let rows = |game: &super::common::TestGame| -> Vec<petramond::net::protocol::MobStateRow> {
        game.server_world()
            .mobs()
            .instances()
            .iter()
            .map(|m| petramond::net::protocol::MobStateRow {
                id: m.id(),
                kind_id: m.kind.0,
                pos: m.pos,
                yaw: 0.0,
                tilt: petramond_math::math::Tilt::LEVEL,
                anim_time: 0.0,
                moving: false,
                idle_anim: None,
                head_yaw: 0.0,
                head_pitch: 0.0,
                hurt_timer: 0.0,
                dead: m.is_dead(),
                shorn: false,
                emitters: Vec::new(),
                conditions: Vec::new(),
                anims: Vec::new(),
                ragdoll: None,
                dig: None,
                held: [None; 2],
                draw: Default::default(),
            })
            .collect()
    };
    let batch = rows(&game);
    game.replica.entities.mobs_mut().apply_snapshot(&batch);

    assert_eq!(
        game.replica
            .closest_mob(game.local.cam.pos, dir, player::REACH)
            .map(|(id, _)| id),
        Some(id),
        "a mob in front within reach is targeted (stable id)"
    );
    assert_eq!(
        game.replica
            .closest_mob(game.local.cam.pos, dir, 1.0)
            .map(|(id, _)| id),
        None,
        "a nearer block (smaller max_dist) occludes the mob"
    );
    // A corpse can't be targeted: the row replicates `dead` on the next batch.
    let cam_pos = game.local.cam.pos;
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .damage_mob(
            id,
            100.0,
            Some(cam_pos),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    let batch = rows(&game);
    game.replica.entities.mobs_mut().apply_snapshot(&batch);
    assert_eq!(
        game.replica
            .closest_mob(game.local.cam.pos, dir, player::REACH),
        None,
        "a dead mob isn't targeted"
    );
}

#[test]
fn closest_mob_targets_the_interpolated_render_pose_not_the_future_row() {
    use petramond::net::protocol::MobStateRow;

    fn row(id: u64, pos: WorldPos) -> MobStateRow {
        MobStateRow {
            id,
            kind_id: Mob::Owl.0,
            pos,
            yaw: 0.0,
            tilt: petramond_math::math::Tilt::LEVEL,
            anim_time: 0.0,
            moving: true,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            emitters: Vec::new(),
            conditions: Vec::new(),
            anims: Vec::new(),
            ragdoll: None,
            dig: None,
            held: [None; 2],
            draw: Default::default(),
        }
    }

    let mut game = game();
    let eye = WorldPos::new(8.0, 66.0, 8.0);
    let dir = Vec3::Z;
    let feet_y = eye.y - 0.35;
    let previous = eye + dir * 2.0;
    let future = eye + dir * 6.0;
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row(42, WorldPos::new(previous.x, feet_y, previous.z))]);
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row(42, WorldPos::new(future.x, feet_y, future.z))]);
    game.replica.entities.clock_mut().start();
    game.replica.entities.clock_mut().advance(TICK_DT * 0.5);

    assert_eq!(
        game.replica
            .closest_mob(eye, dir, player::REACH)
            .map(|(id, _)| id),
        Some(42),
        "the halfway rendered body is still in reach even though curr is not"
    );
}

#[test]
fn a_mob_eases_into_and_out_of_its_gait() {
    use petramond::net::protocol::MobStateRow;
    use petramond_render::GaitClip;

    fn row(moving: bool, anim_time: f32) -> MobStateRow {
        MobStateRow {
            id: 7,
            kind_id: Mob::Owl.0,
            pos: WorldPos::new(8.0, 64.0, 8.0),
            yaw: 0.0,
            tilt: petramond_math::math::Tilt::LEVEL,
            anim_time,
            moving,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            hurt_timer: 0.0,
            dead: false,
            shorn: false,
            emitters: Vec::new(),
            conditions: Vec::new(),
            anims: Vec::new(),
            ragdoll: None,
            dig: None,
            held: [None; 2],
            draw: Default::default(),
        }
    }
    let walk = |game: &crate::game::Game| {
        let entry = game.replica.entities.mobs().iter().next().unwrap();
        entry
            .gait_blend
            .iter()
            .find(|(clip, _, _)| *clip == GaitClip::Walk)
            .map(|(_, weight, phase)| (*weight, *phase))
    };

    let mut game = game();
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row(false, 0.0)]);
    // A step begins: the walk comes in from rest, never at full weight.
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row(true, 0.4)]);
    game.replica.entities.mobs_mut().advance_anim_blends(0.05);
    game.replica.entities.mobs_mut().advance_anim_blends(0.05);
    let (weight, _) = walk(&game).expect("the walk is blending in");
    assert!(
        weight > 0.0 && weight < 1.0,
        "eased in, not snapped: {weight}"
    );
    // And ends mid-stride (the sim's clock resets with the gait): the walk
    // fades from the stride it was in, not from the reset clock.
    game.replica
        .entities
        .mobs_mut()
        .apply_snapshot(&[row(false, 0.0)]);
    game.replica.entities.mobs_mut().advance_anim_blends(0.05);
    let (fading, phase) = walk(&game).expect("the walk is still fading out");
    assert!(fading > 0.0 && fading < weight + 1e-6);
    assert_eq!(phase, 0.4, "it holds the stride it stopped in");
}

#[test]
fn fist_takes_four_hits_to_kill_an_owl() {
    let mut game = game();
    let pos = WorldPos::new(8.0, 64.0, 8.0);
    assert!(game.server_world_mut().mobs_mut().spawn(Mob::Owl, pos, 0.0));
    let id = game.server_world().mobs().instances()[0].id();
    assert_eq!(petramond_world::item::attack_damage(None), (1.0, 1.0));
    let from = pos + Vec3::X;
    for i in 0..3 {
        assert!(
            game.server_world_mut()
                .mobs_mut()
                .damage_mob(
                    id,
                    1.0,
                    Some(from),
                    true,
                    None,
                    &MobDamageFeedback::default()
                )
                .is_none(),
            "fist hit {i} isn't lethal"
        );
        for _ in 0..petramond_world::damage::MOB_DAMAGE_IFRAME_TICKS {
            game.server_world_mut().mobs_mut().tick_damage_immunity();
        }
    }
    assert!(
        game.server_world_mut()
            .mobs_mut()
            .damage_mob(
                id,
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
    let id = game.server_world().mobs().instances()[index].id();
    common::aim_server_at_mob(game, index);
    game.session_mut()
        .input_mut()
        .latch_attack(petramond::server::player::AttackClick {
            mob: Some(id),
            player: None,
        });
}

/// A swing FOLLOWS THROUGH before the hand may attack again, and the CLIENT
/// predicts that gate: mashing the button animates one swing per arc, not one
/// per frame (the prediction used to restart the arc every frame while the
/// server's cooldown quietly ate the presses). The press that lands mid-swing
/// is not thrown away — it is HELD, one deep, and fires by itself the frame
/// the hand comes home.
#[test]
fn a_mashed_attack_queues_one_swing_behind_the_follow_through() {
    let mut game = game();
    let pressed = crate::game::GameInput {
        gameplay_enabled: true,
        attack_clicked: true,
        ..Default::default()
    };
    let idle = crate::game::GameInput {
        attack_clicked: false,
        ..pressed
    };
    let dt = TICK_DT / 4.0;
    let frames = |game: &mut common::TestGame, n: usize, input: &crate::game::GameInput| {
        (0..n).filter(|_| game.tick(dt, input).swung_hand).count()
    };
    let recovery = (ATTACK_COOLDOWN_TICKS * 4) as usize;

    assert_eq!(
        frames(&mut game, recovery, &pressed),
        1,
        "the whole follow-through is one swing, however hard it is mashed"
    );
    // The button is up from here: the swing that follows is the QUEUED press,
    // and it is the only one — a mash is one deep, not a stored volley.
    assert_eq!(
        frames(&mut game, recovery + 2, &idle),
        1,
        "the held press fires by itself, once"
    );
    assert_eq!(
        frames(&mut game, recovery, &idle),
        0,
        "and an empty queue swings nothing"
    );
}

#[test]
fn attack_lands_next_tick_then_locks_out_for_the_cooldown() {
    let mut game = game();
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, WorldPos::new(8.0, 64.0, 8.0), 0.0));
    let mut ev = TickEvents::default();

    // A click resolves on the tick (the tick after it was registered).
    click_attack_at(&mut game, 0);
    game.sim_mut().tick_attack(0, &mut ev);
    assert!(ev.player_at(0).swung_hand, "the click lands on the tick");

    // For the rest of the cooldown, a fresh click each tick lands nothing — even
    // spamming can't beat the gate.
    for _ in 0..ATTACK_COOLDOWN_TICKS - 1 {
        ev.player(0).swung_hand = false;
        click_attack_at(&mut game, 0);
        game.sim_mut().tick_attack(0, &mut ev);
        assert!(
            !ev.player_at(0).swung_hand,
            "locked out during the cooldown"
        );
    }

    // The cooldown has now elapsed, so a pending click connects again.
    ev.player(0).swung_hand = false;
    click_attack_at(&mut game, 0);
    game.sim_mut().tick_attack(0, &mut ev);
    assert!(
        ev.player_at(0).swung_hand,
        "the cooldown elapsed, the next attack lands"
    );

    // Only two fist hits (1 dmg each) landed across all those ticks, so the 4-health
    // owl is still alive: the gate makes a spam-click instakill impossible.
    assert!(
        !game.server_world().mobs().instances()[0].is_dead(),
        "rate-limited, so the owl survives the burst"
    );
}

/// A registered `attack_attempt` handler's Cancel is a CLAIM: the engine's
/// melee stands down (the crosshair's mob is untouched), yet the press was
/// still a swing — the hand swings, the cooldown arms and the Attack edge
/// latches — because whoever took the press owes the hit, not the gesture.
/// A handler that passes leaves the engine's hit exactly as it was.
#[test]
fn a_claimed_attack_attempt_stands_the_melee_down_but_still_swings() {
    let mut game = game();
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, WorldPos::new(8.0, 64.0, 8.0), 0.0));
    let h0 = game.server_world().mobs().instances()[0].health();
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = std::sync::Arc::clone(&seen);
    let claim = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let verdict = std::sync::Arc::clone(&claim);
    game.sim_mut().bus_mut().on_attack_attempt(0, move |_, ev| {
        assert!(
            ev.mob.is_some(),
            "the validated crosshair mob rides the attempt"
        );
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if verdict.load(std::sync::atomic::Ordering::SeqCst) {
            Outcome::Cancel
        } else {
            Outcome::Continue
        }
    });
    let mut ev = TickEvents::default();

    click_attack_at(&mut game, 0);
    game.sim_mut().tick_attack(0, &mut ev);
    assert_eq!(
        seen.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "dispatched once"
    );
    assert_eq!(
        game.server_world().mobs().instances()[0].health(),
        h0,
        "a claimed press lands no engine hit"
    );
    assert!(ev.player_at(0).swung_hand, "but the hand swung");
    assert_eq!(
        game.session().sim().attack_cooldown,
        ATTACK_COOLDOWN_TICKS,
        "and the cooldown armed"
    );
    assert_eq!(
        game.session().replication().swing_events.main,
        Some(mod_api::SwingKind::Attack),
        "and the Attack edge latched for the swing facts"
    );

    // Passing hands the press back to the engine's melee.
    claim.store(false, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..ATTACK_COOLDOWN_TICKS {
        game.sim_mut().tick_attack(0, &mut ev);
    }
    click_attack_at(&mut game, 0);
    game.sim_mut().tick_attack(0, &mut ev);
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        game.server_world().mobs().instances()[0].health() < h0,
        "a passed press is the engine's hit"
    );
}

/// A mod-landed hit naming a player attacker IS that player's melee in its
/// consequences: the attack source with an origin shoves the victim, where
/// the mod's own damage (no attacker) only hurts. The distinction is the
/// whole reason a `DamageMob` can name an attacker.
#[test]
fn a_mod_hit_landed_for_a_player_shoves_like_the_players_own_melee() {
    let mut game = game_on_empty_chunk();
    let mut ev = TickEvents::default();
    let from = WorldPos::new(6.0, 200.0, 8.0);
    for (attack, label) in [(false, "the mod's own damage"), (true, "a player's strike")] {
        assert!(game.server_world_mut().mobs_mut().spawn(
            Mob::Owl,
            WorldPos::new(8.0, 200.0, 8.0),
            0.0
        ));
        let idx = game.server_world().mobs().instances().len() - 1;
        let id = game.server_world().mobs().instances()[idx].id();
        let h0 = game.server_world().mobs().instances()[idx].health();
        let source = if attack {
            DamageSource::PlayerAttack(game.session().id())
        } else {
            DamageSource::Mod("testmod")
        };
        game.sim_mut().bus_mut().queue_mut().push_action(
            petramond::events::DeferredAction::DamageMob {
                mob_id: id,
                amount: 1.0,
                source,
                origin: Some(from),
                feedback: None,
            },
        );
        game.sim_mut().apply_deferred_actions(&mut ev);
        let mob = &game.server_world().mobs().instances()[idx];
        assert!(mob.health() < h0, "{label}: the hit lands");
        assert_eq!(
            mob.staggered(),
            attack,
            "{label}: shoved = {}",
            mob.staggered()
        );
    }
}

#[test]
fn dead_and_spectator_players_cannot_attack_mobs() {
    for spectator in [false, true] {
        let mut game = game_on_empty_chunk();
        assert!(game.server_world_mut().mobs_mut().spawn(
            Mob::Owl,
            WorldPos::new(8.0, 200.0, 8.0),
            0.0
        ));
        click_attack_at(&mut game, 0);
        if spectator {
            game.server_player_mut()
                .set_mode(petramond::player::PlayerMode::Spectator);
        } else {
            game.server_player_mut().set_health(0);
        }
        let health = game.server_world().mobs().instances()[0].health();
        let mut ev = TickEvents::default();

        game.sim_mut().tick_attack(0, &mut ev);

        assert_eq!(
            game.server_world().mobs().instances()[0].health(),
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
    assert!(game.server_world_mut().mobs_mut().spawn(
        Mob::Owl,
        WorldPos::new(8.0, 200.0, 8.0),
        0.0
    ));
    click_attack_at(&mut game, 0);
    let mob_id = game.server_world().mobs().instances()[0].id();
    let player_id = game.session().id().0;
    let health = game.server_world().mobs().instances()[0].health();

    // Placement runs before Attack. A successful board therefore updates the
    // authoritative registry while the session mirror remains stale until
    // the later Riding pass.
    assert!(game.server_world_mut().riding_mut().mount(
        player_id,
        petramond::mob::riding::MountTarget::Mob(mob_id),
        0
    ));
    assert!(game.session().mount().is_none());
    let mut events = TickEvents::default();

    game.sim_mut().tick_attack(0, &mut events);

    assert_eq!(game.server_world().mobs().instances()[0].health(), health);
    assert!(
        events.player_at(0).swung_hand,
        "the rejected own-mount claim degrades to an air punch"
    );
}

#[test]
fn a_forged_mob_id_cannot_redirect_an_attack_past_the_nearest_body() {
    let mut game = game_on_empty_chunk();
    assert!(game.server_world_mut().mobs_mut().spawn(
        Mob::Owl,
        WorldPos::new(8.0, 200.0, 8.0),
        0.0
    ));
    assert!(game.server_world_mut().mobs_mut().spawn(
        Mob::Owl,
        WorldPos::new(8.0, 200.0, 9.0),
        0.0
    ));
    common::aim_server_at_mob(&mut game, 0);
    let forged = game.server_world().mobs().instances()[1].id();
    let health: Vec<_> = game
        .server_world()
        .mobs()
        .instances()
        .iter()
        .map(|mob| mob.health())
        .collect();
    game.session_mut()
        .input_mut()
        .latch_attack(petramond::server::player::AttackClick {
            mob: Some(forged),
            player: None,
        });
    let mut ev = TickEvents::default();

    game.sim_mut().tick_attack(0, &mut ev);

    let after: Vec<_> = game
        .server_world()
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
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, WorldPos::new(8.0, 64.0, 8.0), 0.0));
    let mob_id = game.server_world().mobs().instances()[0].id();

    // A click message latches while playing...
    game.send_to_server(ClientToServer::Action(PlayerAction::AttackClick {
        mob: Some(mob_id),
        player: None,
    }));
    assert!(
        game.session().input().attack().is_some(),
        "the click latched while playing"
    );

    // ...then a screen takes input focus before any tick ran (the next frame's
    // PlayerUpdate reports gameplay=false). The latched press is dropped, so
    // the tick that still runs behind the menu lands no attack.
    let update = common::player_update(&game, false);
    game.send_to_server(ClientToServer::PlayerUpdate(update));
    assert!(
        game.session().input().attack().is_none(),
        "opening a screen drops the latched press"
    );
    assert!(
        game.session().input().attack().is_none(),
        "the click's mob target is dropped with it"
    );
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);
    assert!(
        !ev.player_at(0).swung_hand,
        "no attack fires behind the open menu"
    );
}

#[test]
fn a_killed_mob_ragdolls_then_despawns() {
    let mut game = game_on_empty_chunk();
    let pos = WorldPos::new(8.0, 64.0, 8.0);
    assert!(game.server_world_mut().mobs_mut().spawn(Mob::Owl, pos, 0.0));
    let id = game.server_world().mobs().instances()[0].id();
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .damage_mob(
            id,
            100.0,
            Some(pos + Vec3::X),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    assert_eq!(
        game.server_world().mobs().len(),
        1,
        "the corpse is present while ragdolling"
    );
    let player_pos = game.server_player().body_center();
    let player_body = game.server_player().body();
    // 1.5 s ragdoll lifetime at 20 TPS = 30 ticks; run extra for margin.
    for _ in 0..50 {
        game.server_world_mut().tick_mobs(
            TICK_DT,
            &[petramond::mob::PlayerAnchor {
                pos: player_pos,
                body: Some(player_body),
                ..Default::default()
            }],
        );
    }
    assert_eq!(
        game.server_world().mobs().len(),
        0,
        "the corpse despawns once the ragdoll finishes"
    );
}

#[test]
fn mobs_take_player_rule_fall_damage_when_they_land() {
    let mut game = game();
    game.server_world_mut().clear_world();
    let mut chunk = petramond_world::chunk::Chunk::new(0, 0);
    for z in 0..petramond_world::chunk::CHUNK_SZ {
        for x in 0..petramond_world::chunk::CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Grass);
        }
    }
    game.server_world_mut()
        .insert_chunk_for_test(petramond_world::chunk::ChunkPos::new(0, 0), chunk);

    let spawn = WorldPos::new(8.5, 70.0, 8.5);
    game.server_player_mut().pos = WorldPos::new(8.5, 64.0, 8.5);
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, spawn, 0.0));
    let health0 = game.server_world().mobs().instances()[0].health();
    let player = game.server_player().body_center();
    let body = game.server_player().body();
    let anchors = [petramond::mob::PlayerAnchor {
        id: game.session().id(),
        pos: player,
        body: Some(body),
        ..Default::default()
    }];

    let mut feed = TickEvents::default();
    let mut landed = false;
    for _ in 0..80 {
        let mob_events = game.server_world_mut().tick_mobs(TICK_DT, &anchors);
        landed |= !mob_events.falls.is_empty();
        game.sim_mut()
            .apply_mob_fall_damage(mob_events.falls, &mut feed);
        if landed {
            break;
        }
    }

    assert!(landed, "the mob landed and reported a fall");
    let mob = &game.server_world().mobs().instances()[0];
    let expected = petramond::server::health::fall_damage_health((spawn.y - 64.0) as f32) as f32;
    assert_eq!(expected, 3.0, "fixture is a six-block fall");
    assert_eq!(mob.health(), health0 - expected);
    assert!(!mob.is_dead(), "the owl survives this fall at one health");
}

#[test]
fn killing_owls_drops_loot_into_the_world() {
    let mut game = game_on_empty_chunk();
    let pos = WorldPos::new(8.0, 64.0, 8.0);
    // Over many kills the owl table (50% sticks / 25% coal) virtually always yields
    // something — this proves the death→loot path is wired, without pinning the
    // (freely-editable) table contents.
    for _ in 0..40 {
        assert!(game.server_world_mut().mobs_mut().spawn(Mob::Owl, pos, 0.0));
        let id = game.server_world().mobs().instances().last().unwrap().id();
        if let Some(death) = game.server_world_mut().mobs_mut().damage_mob(
            id,
            100.0,
            Some(pos + Vec3::X),
            true,
            None,
            &MobDamageFeedback::default(),
        ) {
            game.sim_mut().spawn_mob_loot(death);
        }
    }
    assert!(
        !game.server_world().item_entities().is_empty(),
        "killing owls drops loot via the loot table"
    );
}

mod physics_and_pvp;
