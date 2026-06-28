use super::super::tick::{TickEvents, TICK_DT};
use super::super::{GameInput, ATTACK_COOLDOWN_TICKS};
use super::common::{filled_inventory, game, hit, install_empty_chunk};
use crate::block::Block;
use crate::mathh::{IVec3, Vec3};
use crate::mob::Mob;
use crate::player;

#[test]
fn closest_mob_targets_in_front_within_reach_skips_block_occluded_and_corpses() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.cam.pos = Vec3::new(8.0, 66.0, 8.0);
    game.cam.pitch = 0.0; // level look, so the eye ray stays at constant y
    let dir = game.cam.forward();
    // An owl two metres ahead, feet dropped so the eye-level ray crosses its body.
    let mut feet = game.cam.pos + dir * 2.0;
    feet.y -= 0.35;
    assert!(game.world.mobs_mut().spawn(Mob::Owl, feet, 0.0));

    assert_eq!(
        game.closest_mob(game.cam.pos, dir, player::REACH),
        Some(0),
        "a mob in front within reach is targeted"
    );
    assert_eq!(
        game.closest_mob(game.cam.pos, dir, 1.0),
        None,
        "a nearer block (smaller max_dist) occludes the mob"
    );
    // A corpse can't be targeted.
    assert!(game
        .world
        .mobs_mut()
        .hurt_mob(0, 100.0, game.cam.pos)
        .is_some());
    assert_eq!(
        game.closest_mob(game.cam.pos, dir, player::REACH),
        None,
        "a dead mob isn't targeted"
    );
}

#[test]
fn fist_takes_four_hits_to_kill_an_owl() {
    let mut game = game();
    let pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(game.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
    assert_eq!(crate::item::attack_damage(None), (1.0, 1.0));
    let from = pos + Vec3::X;
    for i in 0..3 {
        assert!(
            game.world.mobs_mut().hurt_mob(0, 1.0, from).is_none(),
            "fist hit {i} isn't lethal"
        );
    }
    assert!(
        game.world.mobs_mut().hurt_mob(0, 1.0, from).is_some(),
        "the 4th fist hit kills"
    );
}

#[test]
fn attack_lands_next_tick_then_locks_out_for_the_cooldown() {
    let mut game = game();
    assert!(game
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    game.targeted_mob = Some(0);
    let mut ev = TickEvents::default();

    // A click resolves on the tick (the tick after it was registered).
    game.pending_attack = true;
    game.tick_attack(&mut ev);
    assert!(ev.swung_hand, "the click lands on the tick");

    // For the rest of the cooldown, a fresh click each tick lands nothing — even
    // spamming can't beat the 6-tick gate.
    for _ in 0..ATTACK_COOLDOWN_TICKS - 1 {
        ev.swung_hand = false;
        game.pending_attack = true;
        game.tick_attack(&mut ev);
        assert!(!ev.swung_hand, "locked out during the cooldown");
    }

    // The cooldown has now elapsed, so a pending click connects again.
    ev.swung_hand = false;
    game.pending_attack = true;
    game.tick_attack(&mut ev);
    assert!(ev.swung_hand, "the cooldown elapsed, the next attack lands");

    // Only two fist hits (1 dmg each) landed across all those ticks, so the 4-health
    // owl is still alive: the gate makes a spam-click instakill impossible.
    assert!(
        !game.world.mobs().instances()[0].is_dead(),
        "rate-limited, so the owl survives the burst"
    );
}

#[test]
fn opening_a_screen_drops_a_latched_action_so_it_cant_fire_behind_the_menu() {
    let mut game = game();
    assert!(game
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    game.targeted_mob = Some(0);

    // A click latches while playing...
    let click = GameInput {
        gameplay_enabled: true,
        attack_clicked: true,
        ..Default::default()
    };
    game.capture_intent(&click);
    assert!(game.pending_attack, "the click latched while playing");

    // ...then a screen takes input focus before any tick ran. The latched press is
    // dropped, so the tick that still runs behind the menu lands no attack.
    let menu = GameInput {
        gameplay_enabled: false,
        ..Default::default()
    };
    game.capture_intent(&menu);
    assert!(
        !game.pending_attack,
        "opening a screen drops the latched press"
    );
    let mut ev = TickEvents::default();
    game.tick_attack(&mut ev);
    assert!(!ev.swung_hand, "no attack fires behind the open menu");
}

#[test]
fn a_killed_mob_ragdolls_then_despawns() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(game.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
    assert!(game
        .world
        .mobs_mut()
        .hurt_mob(0, 100.0, pos + Vec3::X)
        .is_some());
    assert_eq!(
        game.world.mobs().len(),
        1,
        "the corpse is present while ragdolling"
    );
    let player_pos = game.player.body_center();
    let player_body = crate::mob::Body::new(game.player.pos, player::HALF_W, player::HEIGHT);
    // 1.5 s ragdoll lifetime at 20 TPS = 30 ticks; run extra for margin.
    for _ in 0..50 {
        game.world.tick_mobs(TICK_DT, player_pos, Some(player_body));
    }
    assert_eq!(
        game.world.mobs().len(),
        0,
        "the corpse despawns once the ragdoll finishes"
    );
}

#[test]
fn killing_owls_drops_loot_into_the_world() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let pos = Vec3::new(8.0, 64.0, 8.0);
    // Over many kills the owl table (50% sticks / 25% coal) virtually always yields
    // something — this proves the death→loot path is wired, without pinning the
    // (freely-editable) table contents.
    for _ in 0..40 {
        assert!(game.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
        let idx = game.world.mobs().len() - 1;
        if let Some(death) = game.world.mobs_mut().hurt_mob(idx, 100.0, pos + Vec3::X) {
            game.spawn_mob_loot(death);
        }
    }
    assert!(
        !game.world.item_entities().is_empty(),
        "killing owls drops loot via the loot table"
    );
}

#[test]
fn a_mob_pushes_the_player_per_frame() {
    // The player is shoved out of an overlapping mob every frame (not on the tick),
    // so the drift is smooth. An owl just east of the player pushes it west.
    let mut game = game();
    game.player.pos = Vec3::new(8.0, 64.0, 8.0);
    assert!(game
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.2, 64.0, 8.0), 0.0));
    let x0 = game.player.pos.x;
    for _ in 0..30 {
        game.apply_mob_push(1.0 / 60.0);
    }
    assert!(
        game.player.pos.x < x0 - 0.05,
        "the owl pushed the player -X, away from it: {x0} -> {}",
        game.player.pos.x
    );
}

#[test]
fn cannot_place_a_solid_block_inside_a_mob() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.player.inventory = filled_inventory(); // a stack of Dirt
    game.player.inventory.set_active(0);
    // Park the player far off so only the mob can block placement here.
    game.player.pos = Vec3::new(100.0, 64.0, 100.0);

    // An owl standing in cell (8, 200, 8), high up and clear of the player.
    assert!(game
        .world
        .mobs_mut()
        .spawn(Mob::Owl, Vec3::new(8.5, 200.0, 8.5), 0.0));

    // Aiming a Dirt block into the owl's cell does nothing: no block lands and the
    // held stack isn't consumed.
    let before = game.player.inventory.selected().unwrap().count;
    game.look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y)); // p = (8, 200, 8)
    assert!(
        !game.try_place(),
        "a solid block can't be placed inside the owl"
    );
    assert_eq!(
        Block::from_id(game.world.chunk_block(8, 200, 8)),
        Block::Air,
        "nothing was placed"
    );
    assert_eq!(
        game.player.inventory.selected().unwrap().count,
        before,
        "the held item wasn't consumed"
    );

    // A cell clear of the owl (and the player) places as usual.
    game.look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y)); // p = (0, 200, 0)
    assert!(game.try_place(), "an empty cell places normally");
    assert_eq!(
        Block::from_id(game.world.chunk_block(0, 200, 0)),
        Block::Dirt
    );
}
