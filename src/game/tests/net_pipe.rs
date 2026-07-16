//! Contract tests for the client→server action pipe:
//! `ServerGame::apply_message` latching semantics — reach validation, the
//! server-side fall tracker, stable-id mob targeting, and the menu-click
//! roundtrip. These drive the message path directly, below `Game::tick`.

use super::super::tick::TickEvents;
use super::common::{self, filled_inventory, game, game_on_empty_chunk};
use crate::block::Block;
use crate::gui::MenuSlot;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, Vec3};
use crate::mob::Mob;
use crate::net::protocol::{ClientToServer, MenuSlotWire, PlayerAction, TargetRef};
use crate::server::health::fall_damage_health;

fn install_test_crafting_recipe(game: &mut super::common::TestGame) {
    game.server.recipes = crate::crafting::Recipes::new(
        vec![crate::crafting::CraftingRecipe::new(
            "test:ordered".into(),
            crate::crafting::CraftingStation::Inventory,
            vec![crate::crafting::CraftingIngredient {
                selector: crate::crafting::IngredientSelector::Item(ItemType::Coal),
                count: 1,
                use_mode: crate::crafting::IngredientUse::Consume,
            }],
            ItemStack::new(ItemType::Stick, 2),
        )],
        Vec::new(),
        Vec::new(),
    );
}

fn apply_update(game: &mut super::common::TestGame, u: crate::net::protocol::PlayerUpdate) {
    game.server
        .apply_message(0, ClientToServer::PlayerUpdate(u));
}

#[test]
fn an_out_of_reach_target_latches_none_and_the_tick_mutates_nothing() {
    let mut game = game_on_empty_chunk();
    game.server.world.set_block_world(8, 63, 8, Block::Stone);
    game.server.sessions[0].player.inventory = filled_inventory(); // Dirt in slot 0

    // Player standing at (8, 64, 8); a target within reach latches. The
    // session anchors at the claim — the reach eye is bounded by the F1
    // drift ring of the server's own integration.
    game.server.sessions[0].player.pos = Vec3::new(8.5, 64.0, 8.5);
    let mut u = common::player_update(&game, true);
    u.transform.pos = Vec3::new(8.5, 64.0, 8.5);
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
    game.server.sessions[0].player.pos = Vec3::new(20.0, 64.0, 20.0);
    let mut far = common::player_update(&game, true);
    far.transform.pos = Vec3::new(20.0, 64.0, 20.0);
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
        ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(TargetRef {
                block: IVec3::new(8, 63, 8),
                normal: IVec3::Y,
            }),
            request_id: None,
            predicted: false,
            jabbed: false,
        }),
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
    let mut game = game_on_empty_chunk();
    // Anchor the session at the drop point: claims must stay NEAR the server's
    // integrated position to be accepted (the F1 anti-teleport bound). A
    // cliff-edge topology — a ledge to stand on and a landing floor one
    // column over — because a grounded claim only counts for the fall
    // tracker when geometry actually holds the feet, and the falling body
    // must not clip the ledge on the way down.
    game.server.world.set_block_world(8, 79, 8, Block::Stone);
    game.server.world.set_block_world(9, 69, 8, Block::Stone);
    game.server.world.set_block_world(10, 69, 8, Block::Stone);
    game.server.sessions[0].player.pos = Vec3::new(8.5, 80.0, 8.5);
    let h0 = game.server.sessions[0].player.health();

    // Grounded on the ledge at y=80, a step off the edge, airborne down to a
    // landing at y=70 one column over: a 10-block fall.
    let at = |game: &super::common::TestGame, x: f32, y: f32, on_ground: bool| {
        let mut u = common::player_update(game, true);
        u.transform.pos = Vec3::new(x, y, 8.5);
        u.on_ground = on_ground;
        u
    };
    let u = at(&game, 8.5, 80.0, true);
    apply_update(&mut game, u);
    game.server.tick_movement(0);
    for y in [78.0, 74.0, 71.0] {
        let u = at(&game, 10.0, y, false);
        apply_update(&mut game, u);
        game.server.tick_movement(0);
    }
    let u = at(&game, 10.0, 70.0, true);
    apply_update(&mut game, u);
    game.server.tick_movement(0);

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
    let mut game = game_on_empty_chunk();
    // A pool at the landing point: the swim probe (feet + 0.6) reads water.
    game.server.world.set_block_world(8, 70, 8, Block::Water);
    // Anchor at the drop point so every claim passes the F1 closeness bound.
    game.server.sessions[0].player.pos = Vec3::new(8.5, 80.0, 8.5);
    let h0 = game.server.sessions[0].player.health();

    let at = |game: &super::common::TestGame, y: f32, on_ground: bool| {
        let mut u = common::player_update(game, true);
        u.transform.pos = Vec3::new(8.5, y, 8.5);
        u.on_ground = on_ground;
        u
    };
    let u = at(&game, 80.0, true);
    apply_update(&mut game, u);
    game.server.tick_movement(0);
    for y in [76.0, 73.0] {
        let u = at(&game, y, false);
        apply_update(&mut game, u);
        game.server.tick_movement(0);
    }
    // Splashdown: grounded (or not — water cancels either way) inside the pool.
    let u = at(&game, 70.0, true);
    apply_update(&mut game, u);
    game.server.tick_movement(0);

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
    let mut game = game_on_empty_chunk();
    let mobs = game.server.world.mobs_mut();
    assert!(mobs.spawn(Mob::Owl, Vec3::new(4.0, 64.0, 4.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, Vec3::new(10.0, 64.0, 10.0), 0.0));
    let second_id = mobs.instances()[1].id();
    let h_before = mobs.instances()[1].health();
    common::aim_server_at_mob(&mut game, 1);

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
            request_id: 1,
        },
    );
    assert_eq!(
        game.server.sessions[0].pending_menu_actions.len(),
        1,
        "the click joined the ordered menu-action queue for the tick"
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

#[test]
fn menu_open_click_craft_and_close_execute_in_wire_order() {
    let mut game = game();
    install_test_crafting_recipe(&mut game);
    let inventory = &mut game.server.sessions[0].player.inventory;
    inventory.add(ItemStack::new(ItemType::Coal, 1));
    inventory.click_slot(0); // begin with the ingredient on the cursor

    game.server
        .apply_message(0, ClientToServer::Action(PlayerAction::OpenInventory));
    game.server.apply_message(
        0,
        ClientToServer::MenuClick {
            slot: MenuSlotWire::Inventory(0),
            button: 0,
            shift: false,
            gather: false,
            request_id: 21,
        },
    );
    game.server.apply_message(
        0,
        ClientToServer::CraftRecipe {
            recipe: "test:ordered".into(),
            bulk: false,
            request_id: 22,
        },
    );
    game.server
        .apply_message(0, ClientToServer::Action(PlayerAction::CloseMenu));

    game.server.tick_menu(0, &mut TickEvents::default());

    assert_eq!(
        game.server.sessions[0].menu.target(),
        crate::game::container::ContainerTarget::None
    );
    assert_eq!(
        common::count_item(&game.server.sessions[0].player.inventory, ItemType::Coal),
        0
    );
    assert_eq!(
        common::count_item(&game.server.sessions[0].player.inventory, ItemType::Stick),
        2
    );
    let outcomes = &game.server.sessions[0].pending_action_outcomes;
    assert!(outcomes
        .iter()
        .any(|outcome| outcome.id == 21 && outcome.accepted));
    assert!(outcomes
        .iter()
        .any(|outcome| outcome.id == 22 && outcome.accepted));
}

#[test]
fn craft_after_close_is_denied_once_without_consuming() {
    let mut game = game();
    install_test_crafting_recipe(&mut game);
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.server
        .apply_message(0, ClientToServer::Action(PlayerAction::OpenInventory));
    game.server
        .apply_message(0, ClientToServer::Action(PlayerAction::CloseMenu));
    game.server.apply_message(
        0,
        ClientToServer::CraftRecipe {
            recipe: "test:ordered".into(),
            bulk: false,
            request_id: 23,
        },
    );

    game.server.tick_menu(0, &mut TickEvents::default());

    assert_eq!(
        common::count_item(&game.server.sessions[0].player.inventory, ItemType::Coal),
        1
    );
    let outcomes: Vec<_> = game.server.sessions[0]
        .pending_action_outcomes
        .iter()
        .filter(|outcome| outcome.id == 23)
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].accepted);
    assert_eq!(
        outcomes[0].reason,
        Some(crate::net::protocol::ActionDenyReason::InvalidSlot)
    );
    assert!(
        !game.server.sessions[0].request_open_inventory,
        "same-tick close cancels the stale open-screen one-shot"
    );
}

#[test]
fn close_then_table_interact_recovers_output_before_opening_the_new_menu() {
    let mut game = game_on_empty_chunk();
    install_test_crafting_recipe(&mut game);
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.server
        .open_crafting_for(0, crate::crafting::CraftingStation::Inventory);
    game.server.apply_message(
        0,
        ClientToServer::CraftRecipe {
            recipe: "test:ordered".into(),
            bulk: false,
            request_id: 24,
        },
    );
    game.server.tick_menu(0, &mut TickEvents::default());
    assert!(game.server.sessions[0].menu.craft_output().is_some());

    game.server
        .apply_message(0, ClientToServer::Action(PlayerAction::CloseMenu));
    let table = IVec3::new(4, 64, 4);
    game.server.world.set_block_world(
        table.x,
        table.y,
        table.z,
        crate::block::Block::CraftingTable,
    );
    game.server.sessions[0].look = Some(common::hit(table, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);
    game.server.tick_menu(0, &mut events);

    assert_eq!(
        common::count_item(&game.server.sessions[0].player.inventory, ItemType::Stick),
        2,
        "the previous output was recovered through close before replacement"
    );
    assert_eq!(
        game.server.sessions[0].menu.target(),
        crate::game::container::ContainerTarget::Table
    );
    assert!(game.server.sessions[0].request_open_table);
    assert!(game.server.sessions[0].menu.craft_output().is_none());
}

#[test]
fn shutdown_recovers_untaken_output_without_waiting_for_a_tick() {
    let mut game = game();
    install_test_crafting_recipe(&mut game);
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.server
        .open_crafting_for(0, crate::crafting::CraftingStation::Inventory);
    game.server.apply_message(
        0,
        ClientToServer::CraftRecipe {
            recipe: "test:ordered".into(),
            bulk: false,
            request_id: 25,
        },
    );
    game.server.tick_menu(0, &mut TickEvents::default());
    assert!(game.server.sessions[0].menu.craft_output().is_some());
    game.server
        .apply_message(0, ClientToServer::Action(PlayerAction::CloseMenu));
    game.server.paused = true;

    game.server.close_sessions_and_save();

    assert_eq!(
        common::count_item(&game.server.sessions[0].player.inventory, ItemType::Stick),
        2
    );
    assert!(game.server.sessions[0].menu.craft_output().is_none());
    assert_eq!(
        game.server.sessions[0].menu.target(),
        crate::game::container::ContainerTarget::None
    );
}

#[test]
fn set_view_distance_moves_the_session_radius_and_only_the_host_moves_the_budget() {
    let mut game = game();
    let server_rd = game.server.world.render_dist;

    // A guest's request moves only its own streaming radius (clamped 4..=64);
    // the server budget is the host's, not the guest's.
    let s = game
        .server
        .add_session_for_test(crate::game::session::spawn_player(1));
    game.server
        .apply_message(s, ClientToServer::SetViewDistance { chunks: 8 });
    assert_eq!(game.server.sessions[s].view_radius, 8);
    assert_eq!(
        game.server.world.render_dist, server_rd,
        "a guest request never moves the server budget"
    );
    game.server
        .apply_message(s, ClientToServer::SetViewDistance { chunks: 2 });
    assert_eq!(game.server.sessions[s].view_radius, 4, "requests clamp low");

    // The HOST's slider is the server setting: its request moves the budget
    // with it, so raising the view distance live actually streams wider.
    game.server
        .apply_message(0, ClientToServer::SetViewDistance { chunks: 12 });
    assert_eq!(game.server.sessions[0].view_radius, 12);
    assert_eq!(game.server.world.render_dist, 12);
}
