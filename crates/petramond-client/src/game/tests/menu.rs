use super::common::{apply_drop_actions, filled_inventory, game, game_on_empty_chunk};
use petramond_world::gui_state::MenuSlot;
use petramond_world::inventory::Inventory;
use petramond_world::item::{ItemStack, ItemType};

#[test]
fn container_edits_apply_on_the_tick_not_the_frame() {
    let mut game = game();
    game.server_player_mut().inventory = filled_inventory();

    game.menu_click(
        MenuSlot::Inventory(0),
        petramond_world::gui_state::PointerButton::Primary,
        false,
        false,
    );
    assert!(
        game.server_player().inventory.cursor().is_none(),
        "the click hasn't applied yet — no cursor pickup this frame"
    );

    game.sim_mut().tick_menu(0, &mut Default::default());
    assert!(
        game.server_player().inventory.cursor().is_some(),
        "the tick applies the container edit (the stack is now on the cursor)"
    );
}

#[test]
fn cursor_has_stack_tracks_the_held_stack() {
    let mut game = game();
    game.server_player_mut().inventory = filled_inventory();
    assert!(!game.cursor_has_stack(), "nothing held initially");
    game.server_player_mut().inventory.click_slot(0);
    game.sync_self_view_for_test();
    assert!(game.cursor_has_stack(), "holding a stack after pickup");
}

#[test]
fn closing_cursor_stack_uses_empty_inventory_slot_after_matching_stacks() {
    let mut game = game();
    let mut slots =
        [Some(ItemStack::new(ItemType::Stone, 64)); petramond_world::inventory::TOTAL_SLOTS];
    slots[4] = None;
    game.server_player_mut().inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), None, 0);

    game.sim_mut().close_cursor_stack_for(0);

    assert!(game.server_player().inventory.cursor().is_none());
    assert_eq!(
        game.server_player().inventory.slot(4),
        Some(&ItemStack::new(ItemType::Dirt, 12))
    );
    apply_drop_actions(&mut game);
    assert!(
        game.server_world().item_entities().is_empty(),
        "stashed cursor stack should not drop"
    );
}

#[test]
fn closing_cursor_stack_queues_a_drop_when_inventory_is_full() {
    let mut game = game();
    let slots =
        [Some(ItemStack::new(ItemType::Stone, 64)); petramond_world::inventory::TOTAL_SLOTS];
    game.server_player_mut().inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), None, 0);

    game.sim_mut().close_cursor_stack_for(0);

    assert!(game.server_player().inventory.cursor().is_none());
    assert!(
        game.server_world().item_entities().is_empty(),
        "drop waits for the next tick"
    );
    apply_drop_actions(&mut game);
    assert_eq!(game.server_world().item_entities().len(), 1);
    assert_eq!(
        game.server_world().item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 12)
    );
}

#[test]
fn closing_cursor_stack_fills_matching_partials_then_drops_leftover() {
    let mut game = game();
    let mut slots =
        [Some(ItemStack::new(ItemType::Stone, 64)); petramond_world::inventory::TOTAL_SLOTS];
    slots[2] = Some(ItemStack::new(ItemType::Dirt, 60));
    slots[10] = Some(ItemStack::new(ItemType::Dirt, 63));
    game.server_player_mut().inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), None, 0);

    game.sim_mut().close_cursor_stack_for(0);

    assert!(game.server_player().inventory.cursor().is_none());
    assert_eq!(
        game.server_player().inventory.slot(2),
        Some(&ItemStack::new(ItemType::Dirt, 64))
    );
    assert_eq!(
        game.server_player().inventory.slot(10),
        Some(&ItemStack::new(ItemType::Dirt, 64))
    );
    assert!(
        game.server_world().item_entities().is_empty(),
        "leftover drop waits for the next tick"
    );
    apply_drop_actions(&mut game);
    assert_eq!(game.server_world().item_entities().len(), 1);
    assert_eq!(
        game.server_world().item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 7)
    );
}

#[test]
fn collect_to_cursor_tops_up_from_hotbar_and_grid() {
    use petramond_world::inventory::{Inventory, TOTAL_SLOTS};
    let mut game = game();
    let mut slots = [None; TOTAL_SLOTS];
    slots[2] = Some(ItemStack::new(ItemType::Dirt, 20));
    slots[petramond_world::inventory::HOTBAR_LEN] = Some(ItemStack::new(ItemType::Dirt, 30));
    slots[5] = Some(ItemStack::new(ItemType::Stone, 64));
    game.server_player_mut().inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 5)), None, 0);

    game.collect_to_cursor();

    assert_eq!(game.server_player().inventory.cursor().unwrap().count, 55);
    assert!(game.server_player().inventory.slot(2).is_none());
    assert!(game
        .server_player()
        .inventory
        .slot(petramond_world::inventory::HOTBAR_LEN)
        .is_none());
    assert_eq!(
        game.server_player().inventory.slot(5).unwrap().item,
        ItemType::Stone
    );
}

#[test]
fn widget_clicks_latch_then_dispatch_to_the_owning_mod_on_the_tick() {
    use petramond_world::gui_state::GuiValue;
    use petramond_world::gui_state::PointerButton;

    let mut game = game();
    game.set_mods_for_test(petramond::modding::ModHost::test_unit_guest_host("modtest"));
    let kind =
        petramond_world::gui_state::intern_kind("modtest:panel").expect("mod kind registers");

    petramond_world::gui_state::gui_state_set(
        &mut game.session_mut().sim_mut().gui_state,
        "modtest:stale".into(),
        GuiValue::I32(9),
    );
    game.sim_mut().open_registered_gui_screen_for(
        0,
        kind,
        Some(petramond_math::math::IVec3::new(1, 2, 3).into()),
    );
    assert!(
        game.session().gui_state().get("modtest:stale").is_none(),
        "opening a mod GUI clears the session state map"
    );

    petramond::menu::slots::declare_widget_for_test("bump");
    let dispatches = |game: &super::common::TestGame| game.mods_for_test().probe(0).1;
    let before = dispatches(&game);

    game.menu_click(
        petramond_world::gui_state::MenuSlot::Widget("bump"),
        PointerButton::Primary,
        false,
        false,
    );
    assert_eq!(dispatches(&game), before, "latching is per-frame pure");

    game.sim_mut().tick_menu(0, &mut Default::default());
    assert_eq!(
        dispatches(&game),
        before + 1,
        "the tick dispatched gui_click to the owning mod"
    );

    game.menu_click(
        petramond_world::gui_state::MenuSlot::Widget("bump"),
        PointerButton::Secondary,
        false,
        false,
    );
    game.sim_mut().tick_menu(0, &mut Default::default());
    assert_eq!(dispatches(&game), before + 1);

    petramond_world::gui_state::gui_state_set(
        &mut game.session_mut().sim_mut().gui_state,
        "modtest:mid".into(),
        GuiValue::F32(0.5),
    );
    game.close_open_menu();
    game.apply_latched_actions_for_test();
    assert!(game.session().gui_state().get("modtest:mid").is_none());

    game.menu_click(
        petramond_world::gui_state::MenuSlot::Widget("bump"),
        PointerButton::Primary,
        false,
        false,
    );
    game.sim_mut().tick_menu(0, &mut Default::default());
    assert_eq!(dispatches(&game), before + 1);
}

#[test]
fn chest_lids_follow_the_viewer_count_not_the_local_menu() {
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(8, 64, 8);
    game.server_world_mut()
        .set_block_world(8, 64, 8, Block::Chest);
    game.server_world_mut()
        .insert_chest(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);
    let replica = &mut game.game.replica.world;
    replica.insert_chunk_for_test(
        petramond_world::chunk::ChunkPos::new(0, 0),
        petramond_world::chunk::Chunk::new(0, 0),
    );
    replica.set_block_world(8, 64, 8, Block::Chest);
    replica.insert_entity_facing(pos, petramond_world::block_model::DEFAULT_MODEL_FACING);

    let mut ev = petramond::events::tick::TickEvents::default();
    game.sim_mut().open_chest_screen_for(0, pos, &mut ev);
    assert_eq!(game.sim().chest_viewers(pos), 1);
    game.sim_mut().open_chest_screen_for(0, pos, &mut ev);
    assert_eq!(game.sim().chest_viewers(pos), 1);

    game.sim_mut().set_chest_viewed_for_test(pos, true, &mut ev);
    game.sim_mut().close_open_menu_for(0, &mut ev);
    assert_eq!(
        game.sim().chest_viewers(pos),
        1,
        "one viewer remains after the local player closes"
    );
    game.sync_open_chests_for_test();
    for _ in 0..30 {
        game.game
            .fx
            .advance_block_animations(&game.game.replica.world, 0.05);
    }
    assert!(
        game.game.fx.block_open_progress(pos, false) > 0.9,
        "the lid stays open while ANY player is looking inside"
    );

    game.sim_mut()
        .set_chest_viewed_for_test(pos, false, &mut ev);
    game.sync_open_chests_for_test();
    for _ in 0..60 {
        game.game
            .fx
            .advance_block_animations(&game.game.replica.world, 0.05);
    }
    assert!(
        game.game.fx.block_open_progress(pos, false) < 0.05,
        "the lid closes once the last viewer leaves"
    );
}
