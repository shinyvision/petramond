use super::common::{self, filled_inventory, game, game_on_empty_chunk};
use petramond::events::tick::TickEvents;
use petramond::mob::Mob;
use petramond::net::protocol::{ClientToServer, MenuSlotWire, PlayerAction, TargetRef};
use petramond::server::health::fall_damage_health;
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::gui_state::MenuSlot;
use petramond_world::item::{ItemStack, ItemType};

fn install_test_crafting_recipe(game: &mut super::common::TestGame) {
    game.sim_mut()
        .install_recipes_for_test(petramond_world::crafting::Recipes::new(
            vec![petramond_world::crafting::CraftingRecipe::new(
                "test:ordered".into(),
                petramond_world::crafting::CraftingStation::Inventory,
                vec![petramond_world::crafting::CraftingIngredient {
                    selector: petramond_world::crafting::IngredientSelector::Item(ItemType::Coal),
                    count: 1,
                    use_mode: petramond_world::crafting::IngredientUse::Consume,
                }],
                ItemStack::new(ItemType::Stick, 2),
            )],
            Vec::new(),
        ));
}

fn apply_update(game: &mut super::common::TestGame, u: petramond::net::protocol::PlayerUpdate) {
    game.send_to_server(ClientToServer::PlayerUpdate(u));
}

#[test]
fn an_out_of_reach_target_latches_none_and_the_tick_mutates_nothing() {
    let mut game = game_on_empty_chunk();
    game.server_world_mut()
        .set_block_world(8, 63, 8, Block::Stone);
    game.server_player_mut().inventory = filled_inventory();

    game.server_player_mut().pos = WorldPos::new(8.5, 64.0, 8.5);
    let mut u = common::player_update(&game, true);
    u.transform.pos = WorldPos::new(8.5, 64.0, 8.5);
    u.target = Some(TargetRef::face(IVec3::new(8, 63, 8), IVec3::Y));
    apply_update(&mut game, u);
    assert!(
        game.session().look().is_some(),
        "an in-reach target latches"
    );

    game.server_player_mut().pos = WorldPos::new(20.0, 64.0, 20.0);
    let mut far = common::player_update(&game, true);
    far.transform.pos = WorldPos::new(20.0, 64.0, 20.0);
    far.target = Some(TargetRef::face(IVec3::new(8, 63, 8), IVec3::Y));
    apply_update(&mut game, far);
    assert!(
        game.session().look().is_none(),
        "a target beyond REACH + 1 latches as no target"
    );

    let held_before = game
        .server_player()
        .inventory
        .selected()
        .expect("holding dirt")
        .count;
    game.send_to_server(ClientToServer::Action(PlayerAction::UseClick {
        mob: None,
        target: Some(TargetRef::face(IVec3::new(8, 63, 8), IVec3::Y)),
        request_id: None,
        predicted: false,
        jabbed: false,
    }));
    let mut ev = TickEvents::default();
    game.sim_mut().tick_place(0, &mut ev);
    assert_eq!(
        Block::from_id(game.server_world().data().chunk_block(8, 64, 8)),
        Block::Air,
        "nothing was placed above the out-of-reach block"
    );
    assert_eq!(
        game.server_player()
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
    game.server_world_mut()
        .set_block_world(8, 79, 8, Block::Stone);
    game.server_world_mut()
        .set_block_world(9, 69, 8, Block::Stone);
    game.server_world_mut()
        .set_block_world(10, 69, 8, Block::Stone);
    game.server_player_mut().pos = WorldPos::new(8.5, 80.0, 8.5);
    let h0 = game.server_player().health();

    let at = |game: &super::common::TestGame, x: f32, y: f32, on_ground: bool| {
        let mut u = common::player_update(game, true);
        u.transform.pos = WorldPos::new(f64::from(x), f64::from(y), 8.5);
        u.on_ground = on_ground;
        u
    };
    let u = at(&game, 8.5, 80.0, true);
    apply_update(&mut game, u);
    game.sim_mut().tick_movement(0);
    for y in [78.0, 74.0, 71.0] {
        let u = at(&game, 10.0, y, false);
        apply_update(&mut game, u);
        game.sim_mut().tick_movement(0);
    }
    let u = at(&game, 10.0, 70.0, true);
    apply_update(&mut game, u);
    game.sim_mut().tick_movement(0);

    let mut ev = TickEvents::default();
    game.sim_mut().tick_fall_damage(0, &mut ev);
    assert_eq!(
        game.server_player().health(),
        h0 - fall_damage_health(10.0),
        "the server-measured 10-block fall deals exactly the physics fall's damage"
    );
    assert!(ev.player_at(0).player_damaged);

    let h1 = game.server_player().health();
    game.sim_mut().tick_fall_damage(0, &mut ev);
    assert_eq!(game.server_player().health(), h1);
}

#[test]
fn landing_in_water_resets_the_fall_and_deals_no_damage() {
    let mut game = game_on_empty_chunk();
    game.server_world_mut()
        .set_block_world(8, 70, 8, Block::Water);
    game.server_player_mut().pos = WorldPos::new(8.5, 80.0, 8.5);
    let h0 = game.server_player().health();

    let at = |game: &super::common::TestGame, y: f32, on_ground: bool| {
        let mut u = common::player_update(game, true);
        u.transform.pos = WorldPos::new(8.5, f64::from(y), 8.5);
        u.on_ground = on_ground;
        u
    };
    let u = at(&game, 80.0, true);
    apply_update(&mut game, u);
    game.sim_mut().tick_movement(0);
    for y in [76.0, 73.0] {
        let u = at(&game, y, false);
        apply_update(&mut game, u);
        game.sim_mut().tick_movement(0);
    }
    let u = at(&game, 70.0, true);
    apply_update(&mut game, u);
    game.sim_mut().tick_movement(0);

    let mut ev = TickEvents::default();
    game.sim_mut().tick_fall_damage(0, &mut ev);
    assert_eq!(
        game.server_player().health(),
        h0,
        "water breaks the fall: no damage"
    );
}

#[test]
fn attack_clicks_resolve_the_stable_mob_id_after_indices_shifted() {
    let mut game = game_on_empty_chunk();
    let mobs = game.server_world_mut().mobs_mut();
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(4.0, 64.0, 4.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(10.0, 64.0, 10.0), 0.0));
    let second_id = mobs.instances()[1].id();
    let first_id = mobs.instances()[0].id();
    let h_before = mobs.instances()[1].health();
    common::aim_server_at_mob(&mut game, 1);

    game.send_to_server(ClientToServer::Action(PlayerAction::AttackClick {
        mob: Some(second_id),
        player: None,
    }));
    assert!(game.server_world_mut().mobs_mut().remove(first_id));
    assert_eq!(
        game.server_world()
            .mobs()
            .instances()
            .iter()
            .position(|m| m.id() == second_id),
        Some(0),
        "the despawn shifted the clicked owl's index"
    );

    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);
    assert!(ev.player_at(0).swung_hand, "the attack landed");
    let survivor = &game.server_world().mobs().instances()[0];
    assert_eq!(survivor.id(), second_id);
    assert!(
        survivor.health() < h_before,
        "the CLICKED owl was hurt, not whichever mob inherited its index"
    );

    let gone_id = second_id + 1_000;
    game.send_to_server(ClientToServer::Action(PlayerAction::AttackClick {
        mob: Some(gone_id),
        player: None,
    }));
    game.session_mut().sim_mut().attack_cooldown = 0;
    let mut ev = TickEvents::default();
    game.sim_mut().tick_attack(0, &mut ev);
    assert!(ev.player_at(0).swung_hand, "a vanished target still swings");
}

#[test]
fn menu_click_messages_latch_then_apply_on_the_tick() {
    let mut game = game();
    game.server_player_mut().inventory = filled_inventory();

    game.send_to_server(ClientToServer::MenuClick {
        slot: MenuSlotWire::from_menu_slot(&MenuSlot::Inventory(0)),
        button: 0,
        shift: false,
        gather: false,
        request_id: 1,
    });
    assert_eq!(
        game.session().input().queued_menu_actions(),
        1,
        "the click joined the ordered menu-action queue for the tick"
    );
    assert!(
        game.server_player().inventory.cursor().is_none(),
        "no mutation before the tick"
    );

    game.sim_mut().tick_menu(0, &mut TickEvents::default());
    assert!(
        game.server_player().inventory.cursor().is_some(),
        "the tick applied the container edit (stack picked onto the cursor)"
    );
}

#[test]
fn menu_open_click_craft_and_close_execute_in_wire_order() {
    let mut game = game();
    install_test_crafting_recipe(&mut game);
    let inventory = &mut game.server_player_mut().inventory;
    inventory.add(ItemStack::new(ItemType::Coal, 1));
    inventory.click_slot(0);

    game.send_to_server(ClientToServer::Action(PlayerAction::OpenInventory));
    game.send_to_server(ClientToServer::MenuClick {
        slot: MenuSlotWire::Inventory(0),
        button: 0,
        shift: false,
        gather: false,
        request_id: 21,
    });
    game.send_to_server(ClientToServer::CraftRecipe {
        recipe: "test:ordered".into(),
        bulk: false,
        request_id: 22,
    });
    game.send_to_server(ClientToServer::Action(PlayerAction::CloseMenu));

    game.sim_mut().tick_menu(0, &mut TickEvents::default());

    assert_eq!(
        game.session().menu().target(),
        crate::game::container::ContainerTarget::None
    );
    assert_eq!(
        common::count_item(&game.server_player().inventory, ItemType::Coal),
        0
    );
    assert_eq!(
        common::count_item(&game.server_player().inventory, ItemType::Stick),
        2
    );
    let outcomes = &game.session().replication().pending_action_outcomes;
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
    game.server_player_mut()
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.send_to_server(ClientToServer::Action(PlayerAction::OpenInventory));
    game.send_to_server(ClientToServer::Action(PlayerAction::CloseMenu));
    game.send_to_server(ClientToServer::CraftRecipe {
        recipe: "test:ordered".into(),
        bulk: false,
        request_id: 23,
    });

    game.sim_mut().tick_menu(0, &mut TickEvents::default());

    assert_eq!(
        common::count_item(&game.server_player().inventory, ItemType::Coal),
        1
    );
    let outcomes: Vec<_> = game
        .session()
        .replication()
        .pending_action_outcomes
        .iter()
        .filter(|outcome| outcome.id == 23)
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].accepted);
    assert_eq!(
        outcomes[0].reason,
        Some(petramond::net::protocol::ActionDenyReason::InvalidSlot)
    );
    assert!(
        game.session().replication().request_open_gui.is_none(),
        "same-tick close cancels the stale open-screen one-shot"
    );
}

#[test]
fn close_then_table_interact_recovers_output_before_opening_the_new_menu() {
    let mut game = game_on_empty_chunk();
    install_test_crafting_recipe(&mut game);
    game.server_player_mut()
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.sim_mut()
        .open_crafting_for(0, petramond_world::crafting::CraftingStation::Inventory);
    game.send_to_server(ClientToServer::CraftRecipe {
        recipe: "test:ordered".into(),
        bulk: false,
        request_id: 24,
    });
    game.sim_mut().tick_menu(0, &mut TickEvents::default());
    assert!(game.session().menu().craft_output().is_some());

    game.send_to_server(ClientToServer::Action(PlayerAction::CloseMenu));
    let table = IVec3::new(4, 64, 4);
    game.server_world_mut().set_block_world(
        table.x,
        table.y,
        table.z,
        petramond_world::block::Block::CraftingTable,
    );
    game.session_mut().input_mut().look = Some(common::hit(table, IVec3::Y));
    game.sim_mut().queue_place_click_for_test(0);
    let mut events = TickEvents::default();
    game.sim_mut().tick_place(0, &mut events);
    game.sim_mut().tick_menu(0, &mut events);

    assert_eq!(
        common::count_item(&game.server_player().inventory, ItemType::Stick),
        2,
        "the previous output was recovered through close before replacement"
    );
    assert_eq!(
        game.session().menu().target(),
        crate::game::container::ContainerTarget::Gui {
            kind: petramond_world::gui_state::GuiKind::CraftingTable,
            anchor: None
        }
    );
    assert_eq!(
        game.session().replication().request_open_gui,
        Some((
            petramond_world::gui_state::GuiKind::CraftingTable,
            Some(table.into())
        ))
    );
    assert!(game.session().menu().craft_output().is_none());
}

#[test]
fn shutdown_recovers_untaken_output_without_waiting_for_a_tick() {
    let mut game = game();
    install_test_crafting_recipe(&mut game);
    game.server_player_mut()
        .inventory
        .add(ItemStack::new(ItemType::Coal, 1));
    game.sim_mut()
        .open_crafting_for(0, petramond_world::crafting::CraftingStation::Inventory);
    game.send_to_server(ClientToServer::CraftRecipe {
        recipe: "test:ordered".into(),
        bulk: false,
        request_id: 25,
    });
    game.sim_mut().tick_menu(0, &mut TickEvents::default());
    assert!(game.session().menu().craft_output().is_some());
    game.send_to_server(ClientToServer::Action(PlayerAction::CloseMenu));
    game.sim_mut().set_paused_for_test(true);

    game.sim_mut().close_sessions_and_save();

    assert_eq!(
        common::count_item(&game.server_player().inventory, ItemType::Stick),
        2
    );
    assert!(game.session().menu().craft_output().is_none());
    assert_eq!(
        game.session().menu().target(),
        crate::game::container::ContainerTarget::None
    );
}

#[test]
fn set_view_distance_moves_the_session_radius_and_only_the_host_moves_the_budget() {
    let mut game = game();
    let server_rd = game.server_world().data().render_dist;

    let s = game
        .sim_mut()
        .add_session_for_test(petramond::server::session_build::spawn_player(1));
    game.sim_mut()
        .apply_message(s, ClientToServer::SetViewDistance { chunks: 8 });
    assert_eq!(game.session_at(s).transport().view_radius, 8);
    assert_eq!(
        game.server_world().data().render_dist,
        server_rd,
        "a guest request never moves the server budget"
    );
    game.sim_mut()
        .apply_message(s, ClientToServer::SetViewDistance { chunks: 2 });
    assert_eq!(
        game.session_at(s).transport().view_radius,
        4,
        "requests clamp low"
    );

    game.send_to_server(ClientToServer::SetViewDistance { chunks: 12 });
    assert_eq!(game.session().transport().view_radius, 12);
    assert_eq!(game.server_world().data().render_dist, 12);
}
