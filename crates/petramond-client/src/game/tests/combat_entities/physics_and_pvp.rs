use super::*;

#[test]
fn a_mob_pushes_the_player_per_frame() {
    // The player is shoved out of an overlapping mob every frame (not on the tick),
    // so the drift is smooth. An owl just east of the player pushes it west.
    // The push acts on the CLIENT's predicted player against the REPLICATED
    // mob rows (the shove reaches the server in the next PlayerUpdate).
    let mut game = game();
    game.local.player.pos = WorldPos::new(8.0, 64.0, 8.0);
    game.replica.entities
        .mobs_mut()
        .apply_snapshot(&[petramond::net::protocol::MobStateRow {
            id: 1,
            kind_id: Mob::Owl.0,
            pos: WorldPos::new(8.2, 64.0, 8.0),
            yaw: 0.0,
            tilt: petramond_math::math::Tilt::LEVEL,
            anim_time: 0.0,
            moving: false,
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
        }]);
    let x0 = game.local.player.pos.x;
    for _ in 0..30 {
        game.apply_entity_push(1.0 / 60.0);
    }
    assert!(
        game.local.player.pos.x < x0 - 0.05,
        "the owl pushed the player -X, away from it: {x0} -> {}",
        game.local.player.pos.x
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
    use petramond::net::protocol::PlayerStateRow;
    use petramond::player::PlayerId;

    fn remote_row(pos: WorldPos, visible: bool, sleeping: bool) -> PlayerStateRow {
        PlayerStateRow {
            conditions: Vec::new(),
            id: PlayerId(1),
            transform: petramond::net::protocol::Transform {
                pos,
                vel: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
            },
            on_ground: true,
            sneaking: false,
            sleeping,
            sleep_yaw: None,
            alive: visible,
            visible,
            held_item: None,
            held_data: None,
            off_hand_item: None,
            off_hand_data: None,
            mining: None,
            eating: false,
            eating_off_hand: false,
            held_pose_main: None,
            held_pose_off: None,
            held_display: [None; 2],
            bone_poses: Vec::new(),
            animator: Default::default(),
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    let mut game = game();
    let own_id = game.game.replica.entities.self_id();
    let start = WorldPos::new(8.0, 64.0, 8.0);
    let overlap = WorldPos::new(8.2, 64.0, 8.0); // just east, footprints overlapping

    let run = |game: &mut common::TestGame, row: PlayerStateRow| {
        game.local.player.pos = start;
        game.game.replica.entities.players_mut().apply_snapshot(&[row], &[], own_id);
        for _ in 0..30 {
            game.apply_entity_push(1.0 / 60.0);
        }
        game.local.player.pos.x - start.x
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
    game.server_player_mut().inventory = filled_inventory(); // a stack of Dirt
    game.server_player_mut().inventory.set_active(0);
    // Park the player far off so only the mob can block placement here.
    game.server_player_mut().pos = WorldPos::new(100.0, 64.0, 100.0);

    // An owl standing in cell (8, 200, 8), high up and clear of the player.
    assert!(game
        .server_world_mut()
        .mobs_mut()
        .spawn(Mob::Owl, WorldPos::new(8.5, 200.0, 8.5), 0.0));

    // Aiming a Dirt block into the owl's cell does nothing: no block lands and the
    // held stack isn't consumed.
    let before = game.server_player()
        .inventory
        .selected()
        .unwrap()
        .count;
    game.session_mut().look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y)); // p = (8, 200, 8)
    assert!(
        !game.sim_mut().try_place_for_test(),
        "a solid block can't be placed inside the owl"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(8, 200, 8)),
        Block::Air,
        "nothing was placed"
    );
    assert_eq!(
        game.server_player()
            .inventory
            .selected()
            .unwrap()
            .count,
        before,
        "the held item wasn't consumed"
    );

    // A cell clear of the owl (and the player) places as usual.
    game.session_mut().look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y)); // p = (0, 200, 0)
    assert!(
        game.sim_mut().try_place_for_test(),
        "an empty cell places normally"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(0, 200, 0)),
        Block::Dirt
    );
}

#[test]
fn cannot_place_a_solid_block_inside_another_player() {
    let mut game = game_on_empty_chunk();
    game.server_player_mut().inventory = filled_inventory(); // a stack of Dirt
    game.server_player_mut().inventory.set_active(0);
    // Park the placer far off so only the other session can block placement here.
    game.server_player_mut().pos = WorldPos::new(100.0, 64.0, 100.0);

    let other = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(WorldPos::new(
            8.5, 200.0, 8.5,
        )));

    let before = game.server_player()
        .inventory
        .selected()
        .unwrap()
        .count;
    game.session_mut().look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y)); // p = (8, 200, 8)
    assert!(
        !game.sim_mut().try_place_for_test(),
        "a solid block can't be placed inside another live player"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(8, 200, 8)),
        Block::Air,
        "nothing was placed"
    );
    assert_eq!(
        game.server_player()
            .inventory
            .selected()
            .unwrap()
            .count,
        before,
        "the held item wasn't consumed"
    );

    game.session_at_mut(other)
        .player
        .set_mode(petramond::player::PlayerMode::Spectator);
    game.session_mut().look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y));
    assert!(
        game.sim_mut().try_place_for_test(),
        "a spectator has no placement-blocking body"
    );
    assert_eq!(
        Block::from_id(game.server_world().chunk_block(8, 200, 8)),
        Block::Dirt
    );
    assert_eq!(
        game.server_player()
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
fn click_attack_player(game: &mut super::super::common::TestGame, target: petramond::player::PlayerId) {
    game.send_to_server(ClientToServer::Action(PlayerAction::AttackClick {
            mob: None,
            player: Some(target.0),
        }),
    );
}

/// Two sessions in reach; a fist guarantees the deterministic (1.0, 1.0)
/// damage roll.
fn pvp_pair(game: &mut super::super::common::TestGame) -> usize {
    game.server_player_mut().pos = WorldPos::new(0.5, 64.0, 0.5);
    game.server_player_mut().inventory = petramond_world::inventory::Inventory::new();
    let t = game
        .sim_mut()
        .add_session_for_test(petramond::player::Player::new(WorldPos::new(
            2.5, 64.0, 0.5,
        )));
    game.session_at_mut(t).player.vel = Vec3::ZERO;
    t
}

#[test]
fn a_pvp_attack_damages_the_target_through_the_funnel_with_knockback_and_cooldown() {
    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.session_at(t).id;
    let h0 = game.session_at(t).player.health();
    let attacker_h0 = game.server_player().health();

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);

    assert!(ev.player_at(0).swung_hand, "the hit swings the hand");
    assert_eq!(
        game.session().attack_cooldown, ATTACK_COOLDOWN_TICKS,
        "the swing arms the cooldown, exactly like a mob hit"
    );
    assert_eq!(
        game.session_at(t).player.health(),
        h0 - 1,
        "a fist hit costs the target one half-heart"
    );
    assert!(
        ev.player_at(t).player_damaged,
        "the victim's damaged one-shot fires (hurt sound/shake/hurt_recent)"
    );
    assert_eq!(
        game.server_player().health(),
        attacker_h0,
        "only the target is damaged"
    );
    let vel = game.session_at(t).player.vel;
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
    game.session_at_mut(t).player.pos = WorldPos::new(20.5, 64.0, 0.5); // beyond REACH + 1
    let h0 = game.session_at(t).player.health();
    let target_id = game.session_at(t).id;

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);

    assert_eq!(game.session_at(t).player.health(), h0, "no damage");
    assert_eq!(
        game.session_at(t).player.vel,
        Vec3::ZERO,
        "no knockback"
    );
}

#[test]
fn spectators_neither_attack_nor_take_pvp_hits() {
    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.session_at(t).id;

    // A spectator TARGET can't be hit.
    game.session_at_mut(t)
        .player
        .set_mode(petramond::player::PlayerMode::Spectator);
    let h0 = game.session_at(t).player.health();
    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);
    assert_eq!(game.session_at(t).player.health(), h0);
    assert_eq!(game.session_at(t).player.vel, Vec3::ZERO);

    // A spectator ATTACKER can't hit.
    game.session_at_mut(t)
        .player
        .set_mode(petramond::player::PlayerMode::Survival);
    game.server_player_mut()
        .set_mode(petramond::player::PlayerMode::Spectator);
    game.session_mut().attack_cooldown = 0;
    let h0 = game.session_at(t).player.health();
    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);
    assert_eq!(game.session_at(t).player.health(), h0);
}

#[test]
fn a_cancelled_player_damage_pre_suppresses_pvp_damage_and_knockback() {
    use std::sync::{Arc, Mutex};

    let mut game = game();
    let t = pvp_pair(&mut game);
    let target_id = game.session_at(t).id;
    let attacker_id = game.session().id;
    let seen = Arc::new(Mutex::new(None));
    {
        let seen = seen.clone();
        game.sim_mut().bus.on_player_damage_pre(0, move |_, pre| {
            *seen.lock().unwrap() = Some(pre.source);
            Outcome::Cancel
        });
    }
    let h0 = game.session_at(t).player.health();

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);

    assert_eq!(
        game.session_at(t).player.health(),
        h0,
        "cancel = no damage"
    );
    assert_eq!(
        game.session_at(t).player.vel,
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
    let target_id = game.session_at(t).id;
    // What the victim's client last claimed: its exact pre-hit transform.
    let reported = {
        let p = &game.session_at(t).player;
        petramond::net::protocol::SelfTransform {
            transform: petramond::net::protocol::Transform {
                pos: p.pos,
                vel: p.vel,
                yaw: p.yaw,
                pitch: p.pitch,
            },
            on_ground: p.on_ground,
        }
    };
    game.session_at_mut(t).last_reported_transform = Some(reported);

    click_attack_player(&mut game, target_id);
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);

    let state = game.sim_mut().build_self_state(t);
    let echo = state
        .transform
        .expect("a vel-only knockback still ships the transform correction");
    assert_eq!(
        echo.transform.pos, reported.transform.pos,
        "the tick moved no position"
    );
    assert_ne!(
        echo.transform.vel, reported.transform.vel,
        "the echo carries the knocked velocity"
    );
    assert_eq!(
        echo.transform.vel, game.session_at(t).player.vel,
        "the echoed velocity is the session's post-knockback one"
    );
}

/// Client-side PvP targeting: a visible, alive remote body under the
/// crosshair is targeted (nearest wins vs mobs; at most one target kind is
/// set); dead/invisible remotes are ignored.
#[test]
fn refresh_target_picks_remote_players_competing_with_mobs() {
    use petramond::net::protocol::PlayerStateRow;
    use petramond::player::PlayerId;

    fn remote_row(id: u8, pos: WorldPos, visible: bool) -> PlayerStateRow {
        PlayerStateRow {
            conditions: Vec::new(),
            id: PlayerId(id),
            transform: petramond::net::protocol::Transform {
                pos,
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
            held_data: None,
            off_hand_item: None,
            off_hand_data: None,
            mining: None,
            eating: false,
            eating_off_hand: false,
            held_pose_main: None,
            held_pose_off: None,
            held_display: [None; 2],
            bone_poses: Vec::new(),
            animator: Default::default(),
            hurt_recent: false,
            snap: false,
            mount: None,
        }
    }

    let mut game = game_on_empty_chunk();
    game.local.cam.pos = WorldPos::new(8.0, 66.0, 8.0);
    game.local.cam.pitch = 0.0;
    let dir = game.local.cam.forward();
    let own_id = game.game.replica.entities.self_id();

    // A remote body two metres ahead, feet dropped so the level ray crosses it.
    let mut feet = game.local.cam.pos + dir * 2.0;
    feet.y -= 1.0;
    game.game
        .replica.entities
        .players_mut()
        .apply_snapshot(&[remote_row(1, feet, true)], &[], own_id);
    game.refresh_target();
    assert_eq!(game.local.targeted_player, Some(1), "the remote body is targeted");
    assert!(game.local.targeted_mob.is_none(), "at most one target kind");
    assert!(
        game.local.look.is_none(),
        "an entity target clears the block look"
    );

    // A mob NEARER than the remote wins the distance competition.
    let mut mob_feet = game.local.cam.pos + dir * 1.2;
    mob_feet.y -= 0.35;
    game.game
        .replica.entities
        .mobs_mut()
        .apply_snapshot(&[petramond::net::protocol::MobStateRow {
            id: 42,
            kind_id: Mob::Owl.0,
            pos: mob_feet,
            yaw: 0.0,
            tilt: petramond_math::math::Tilt::LEVEL,
            anim_time: 0.0,
            moving: false,
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
        }]);
    game.refresh_target();
    assert_eq!(game.local.targeted_mob, Some(42), "the nearer mob wins");
    assert!(game.local.targeted_player.is_none());

    // A hidden (dead/spectator) remote is never targeted.
    game.game.replica.entities.mobs_mut().apply_snapshot(&[]);
    game.game
        .replica.entities
        .players_mut()
        .apply_snapshot(&[remote_row(1, feet, false)], &[], own_id);
    game.refresh_target();
    assert!(
        game.local.targeted_player.is_none(),
        "hidden bodies are untargetable"
    );
}

/// A frame hitch (opening the inventory, the ESC pause transition) pumps
/// SEVERAL fixed ticks at once, and a fast fall legitimately covers more
/// than two blocks across them. The pump's teleport detector must scale its
/// discontinuity bound with the ticks it actually ran — a fixed bound reset
/// the fall tracker mid-fall, so flashing a menu while falling landed
/// without damage.
#[test]
fn multi_tick_pumps_never_eat_fall_damage() {
    let mut game = game();
    common::flat_floor_loaded_air(game.server_world_mut(), Block::Stone);
    let sess = game.session_mut();
    sess.player.pos = WorldPos::new(8.5, 79.0, 8.5);
    sess.player.vel = Vec3::ZERO;
    sess.player.on_ground = false;
    sess.fall.reset(79.0);
    let start = sess.player.health();

    // Every "frame" runs three fixed ticks — the hitchy cadence a menu
    // open produces. The fall is pure server F2 integration (no claims).
    for _ in 0..300 {
        game.pump_server(3.0 * TICK_DT);
        if game.server_player().on_ground {
            break;
        }
    }
    assert!(
        game.server_player().on_ground,
        "the drop lands on the floor"
    );
    // One more hitchy frame so the landing tick's pending fall drains even
    // if it straddled the pump boundary.
    game.pump_server(3.0 * TICK_DT);

    let lost = start - game.server_player().health();
    assert!(
        lost >= 8,
        "a ~15-block fall through multi-tick pumps lands its damage (lost {lost} half-hearts)"
    );
}
