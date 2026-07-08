//! Contract tests for the client→server action pipe (multiplayer Phase C2b):
//! `ServerGame::apply_message` latching semantics — reach validation, the
//! server-side fall tracker, stable-id mob targeting, and the menu-click
//! roundtrip. These drive the message path directly, below `Game::tick`.

use super::super::tick::TickEvents;
use super::common::{self, filled_inventory, game, install_empty_chunk};
use crate::block::Block;
use crate::gui::MenuSlot;
use crate::mathh::{IVec3, Vec3};
use crate::mob::Mob;
use crate::net::protocol::{ClientToServer, MenuSlotWire, PlayerAction, TargetRef};
use crate::server::health::fall_damage_health;

fn apply_update(game: &mut super::common::TestGame, u: crate::net::protocol::PlayerUpdate) {
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
}

#[test]
fn an_out_of_reach_target_latches_none_and_the_tick_mutates_nothing() {
    let mut game = game();
    install_empty_chunk(&mut game);
    game.server.world.set_block_world(8, 63, 8, Block::Stone);
    game.server.sessions[0].player.inventory = filled_inventory(); // Dirt in slot 0

    // Player standing at (8, 64, 8); a target within reach latches.
    let mut u = common::player_update(&game, true);
    u.pos = Vec3::new(8.5, 64.0, 8.5);
    u.target = Some(TargetRef {
        block: IVec3::new(8, 63, 8),
        normal: IVec3::Y,
    });
    apply_update(&mut game, u);
    assert!(
        game.server.sessions[0].look.is_some(),
        "an in-reach target latches"
    );

    // The same target reported from far away is silently dropped...
    let mut far = common::player_update(&game, true);
    far.pos = Vec3::new(20.0, 64.0, 20.0);
    far.target = Some(TargetRef {
        block: IVec3::new(8, 63, 8),
        normal: IVec3::Y,
    });
    apply_update(&mut game, far);
    assert!(
        game.server.sessions[0].look.is_none(),
        "a target beyond REACH + 1 latches as no target"
    );

    // ...so the use click that follows it places nothing and keeps the item.
    let held_before = game.server.sessions[0]
        .player
        .inventory
        .selected()
        .expect("holding dirt")
        .count;
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::UseClick { mob: None }),
    );
    let mut ev = TickEvents::default();
    game.server.tick_place(0, &mut ev);
    assert_eq!(
        Block::from_id(game.server.world.chunk_block(8, 64, 8)),
        Block::Air,
        "nothing was placed above the out-of-reach block"
    );
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .selected()
            .expect("still holding dirt")
            .count,
        held_before,
        "the held item was not consumed"
    );
}

#[test]
fn a_reported_fall_deals_the_same_damage_the_physics_fall_would() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let h0 = game.server.sessions[0].player.health();

    // Grounded at y=80, airborne down to a landing at y=70: a 10-block fall.
    let at = |game: &super::common::TestGame, y: f32, on_ground: bool| {
        let mut u = common::player_update(game, true);
        u.pos = Vec3::new(8.5, y, 8.5);
        u.on_ground = on_ground;
        u
    };
    let u = at(&game, 80.0, true);
    apply_update(&mut game, u);
    for y in [78.0, 74.0, 71.0] {
        let u = at(&game, y, false);
        apply_update(&mut game, u);
    }
    let u = at(&game, 70.0, true);
    apply_update(&mut game, u);

    let mut ev = TickEvents::default();
    game.server.tick_fall_damage(0, &mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        h0 - fall_damage_health(10.0),
        "the server-measured 10-block fall deals exactly the physics fall's damage"
    );
    assert!(ev.player_at(0).player_damaged);

    // A second consume finds nothing: the landing was a one-shot.
    let h1 = game.server.sessions[0].player.health();
    game.server.tick_fall_damage(0, &mut ev);
    assert_eq!(game.server.sessions[0].player.health(), h1);
}

#[test]
fn landing_in_water_resets_the_fall_and_deals_no_damage() {
    let mut game = game();
    install_empty_chunk(&mut game);
    // A pool at the landing point: the swim probe (feet + 0.6) reads water.
    game.server.world.set_block_world(8, 70, 8, Block::Water);
    let h0 = game.server.sessions[0].player.health();

    let at = |game: &super::common::TestGame, y: f32, on_ground: bool| {
        let mut u = common::player_update(game, true);
        u.pos = Vec3::new(8.5, y, 8.5);
        u.on_ground = on_ground;
        u
    };
    let u = at(&game, 80.0, true);
    apply_update(&mut game, u);
    for y in [76.0, 73.0] {
        let u = at(&game, y, false);
        apply_update(&mut game, u);
    }
    // Splashdown: grounded (or not — water cancels either way) inside the pool.
    let u = at(&game, 70.0, true);
    apply_update(&mut game, u);

    let mut ev = TickEvents::default();
    game.server.tick_fall_damage(0, &mut ev);
    assert_eq!(
        game.server.sessions[0].player.health(),
        h0,
        "water breaks the fall: no damage"
    );
}

#[test]
fn attack_clicks_resolve_the_stable_mob_id_after_indices_shifted() {
    let mut game = game();
    install_empty_chunk(&mut game);
    let mobs = game.server.world.mobs_mut();
    assert!(mobs.spawn(Mob::Owl, Vec3::new(4.0, 64.0, 4.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, Vec3::new(10.0, 64.0, 10.0), 0.0));
    let second_id = mobs.instances()[1].id();
    let h_before = mobs.instances()[1].health();

    // Click the second owl, then despawn the FIRST before the tick —
    // swap_remove renumbers the second owl into index 0.
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::AttackClick {
            mob: Some(second_id),
            player: None,
        }),
    );
    assert!(game.server.world.mobs_mut().remove(0));
    assert_eq!(
        game.server.world.mobs().index_of_id(second_id),
        Some(0),
        "the despawn shifted the clicked owl's index"
    );

    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);
    assert!(ev.player_at(0).swung_hand, "the attack landed");
    let survivor = &game.server.world.mobs().instances()[0];
    assert_eq!(survivor.id(), second_id);
    assert!(
        survivor.health() < h_before,
        "the CLICKED owl was hurt, not whichever mob inherited its index"
    );

    // A click on a mob that vanished entirely degrades to an air punch: the
    // hand swings, nothing is hurt.
    let gone_id = second_id + 1_000;
    game.server.apply_message(
        0,
        ClientToServer::Action(PlayerAction::AttackClick {
            mob: Some(gone_id),
            player: None,
        }),
    );
    game.server.sessions[0].attack_cooldown = 0;
    let mut ev = TickEvents::default();
    game.server.tick_attack(0, &mut ev);
    assert!(ev.player_at(0).swung_hand, "a vanished target still swings");
}

#[test]
fn menu_click_messages_latch_then_apply_on_the_tick() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory(); // Dirt in slot 0

    game.server.apply_message(
        0,
        ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&MenuSlot::Inventory(0)),
            button: 0, // primary
            shift: false,
            gather: false,
        },
    );
    assert_eq!(
        game.server.sessions[0].pending_menu_clicks.len(),
        1,
        "the click latched for the tick"
    );
    assert!(
        game.server.sessions[0].player.inventory.cursor().is_none(),
        "no mutation before the tick"
    );

    game.server.tick_menu(0, &mut TickEvents::default());
    assert!(
        game.server.sessions[0].player.inventory.cursor().is_some(),
        "the tick applied the container edit (stack picked onto the cursor)"
    );
}
