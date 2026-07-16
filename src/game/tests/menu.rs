use super::common::{apply_drop_actions, filled_inventory, game, game_on_empty_chunk};
use crate::gui::MenuSlot;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};

#[test]
fn container_edits_apply_on_the_tick_not_the_frame() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory(); // a stack of Dirt in hotbar slot 0

    // Left-click that slot: it should pick the stack onto the cursor — but that's a
    // container edit, so it's latched, not applied this frame.
    game.menu_click(
        MenuSlot::Inventory(0),
        crate::controls::PointerButton::Primary,
        false,
        false,
    );
    assert!(
        game.server.sessions[0].player.inventory.cursor().is_none(),
        "the click hasn't applied yet — no cursor pickup this frame"
    );

    // The tick applies it, moving the stack onto the cursor.
    game.server.tick_menu(0, &mut Default::default());
    assert!(
        game.server.sessions[0].player.inventory.cursor().is_some(),
        "the tick applies the container edit (the stack is now on the cursor)"
    );
}

#[test]
fn cursor_has_stack_tracks_the_held_stack() {
    let mut game = game();
    game.server.sessions[0].player.inventory = filled_inventory();
    assert!(!game.cursor_has_stack(), "nothing held initially");
    game.server.sessions[0].player.inventory.click_slot(0); // pick up hotbar slot 0
    game.sync_self_view_for_test(); // the read is replicated (SelfView.inventory)
    assert!(game.cursor_has_stack(), "holding a stack after pickup");
}

#[test]
fn closing_cursor_stack_uses_empty_inventory_slot_after_matching_stacks() {
    let mut game = game();
    let mut slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
    slots[4] = None;
    game.server.sessions[0].player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

    game.server.close_cursor_stack_for(0);

    assert!(game.server.sessions[0].player.inventory.cursor().is_none());
    assert_eq!(
        game.server.sessions[0].player.inventory.slot(4),
        Some(&ItemStack::new(ItemType::Dirt, 12))
    );
    apply_drop_actions(&mut game);
    assert!(
        game.server.world.item_entities().is_empty(),
        "stashed cursor stack should not drop"
    );
}

#[test]
fn closing_cursor_stack_queues_a_drop_when_inventory_is_full() {
    let mut game = game();
    let slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
    game.server.sessions[0].player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

    game.server.close_cursor_stack_for(0);

    assert!(game.server.sessions[0].player.inventory.cursor().is_none());
    assert!(
        game.server.world.item_entities().is_empty(),
        "drop waits for the next tick"
    );
    apply_drop_actions(&mut game);
    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        game.server.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 12)
    );
}

#[test]
fn closing_cursor_stack_fills_matching_partials_then_drops_leftover() {
    let mut game = game();
    let mut slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
    slots[2] = Some(ItemStack::new(ItemType::Dirt, 60));
    slots[10] = Some(ItemStack::new(ItemType::Dirt, 63));
    game.server.sessions[0].player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

    game.server.close_cursor_stack_for(0);

    assert!(game.server.sessions[0].player.inventory.cursor().is_none());
    assert_eq!(
        game.server.sessions[0].player.inventory.slot(2),
        Some(&ItemStack::new(ItemType::Dirt, 64))
    );
    assert_eq!(
        game.server.sessions[0].player.inventory.slot(10),
        Some(&ItemStack::new(ItemType::Dirt, 64))
    );
    assert!(
        game.server.world.item_entities().is_empty(),
        "leftover drop waits for the next tick"
    );
    apply_drop_actions(&mut game);
    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        game.server.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 7)
    );
}

#[test]
fn collect_to_cursor_tops_up_from_hotbar_and_grid() {
    use crate::inventory::{Inventory, TOTAL_SLOTS};
    let mut game = game();
    // Cursor holds a partial Dirt stack; matching partials sit in the hotbar
    // and the main grid, with an unrelated stack that must be left alone.
    let mut slots = [None; TOTAL_SLOTS];
    slots[2] = Some(ItemStack::new(ItemType::Dirt, 20)); // hotbar
    slots[crate::inventory::HOTBAR_LEN] = Some(ItemStack::new(ItemType::Dirt, 30)); // main grid
    slots[5] = Some(ItemStack::new(ItemType::Stone, 64)); // untouched
    game.server.sessions[0].player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 5)), 0);

    game.collect_to_cursor();

    // 5 + 20 + 30 = 55 onto the cursor, both dirt sources emptied.
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .cursor()
            .unwrap()
            .count,
        55
    );
    assert!(game.server.sessions[0].player.inventory.slot(2).is_none());
    assert!(game.server.sessions[0]
        .player
        .inventory
        .slot(crate::inventory::HOTBAR_LEN)
        .is_none());
    assert_eq!(
        game.server.sessions[0]
            .player
            .inventory
            .slot(5)
            .unwrap()
            .item,
        ItemType::Stone
    );
}

/// The Phase 5 mod-GUI session contract on the Game side: opening clears the
/// state map, a button click LATCHES (per-frame pure) and only the tick
/// dispatches a `gui_click` to the OWNING mod (namespace prefix == pack id),
/// a secondary click over a button triggers nothing, and closing the session
/// clears the map again.
#[test]
fn widget_clicks_latch_then_dispatch_to_the_owning_mod_on_the_tick() {
    use crate::controls::PointerButton;
    use crate::gui::GuiValue;

    let mut game = game();
    game.set_mods_for_test(crate::modding::ModHost::test_unit_guest_host("modtest"));
    let kind = crate::gui::intern_kind("modtest:panel").expect("mod kind registers");

    // Stale values from before the session must not survive the open.
    crate::gui::gui_state_set(
        &mut game.server.sessions[0].gui_state,
        "modtest:stale".into(),
        GuiValue::I32(9),
    );
    game.server
        .open_mod_gui_screen_for(0, kind, Some(crate::mathh::IVec3::new(1, 2, 3)));
    assert!(
        game.server.sessions[0]
            .gui_state
            .get("modtest:stale")
            .is_none(),
        "opening a mod GUI clears the session state map"
    );

    let dispatches = |game: &super::common::TestGame| game.mods_for_test().probe(0).1;
    let before = dispatches(&game);

    // The click latches this frame; nothing dispatches until the tick.
    game.menu_click(
        crate::gui::MenuSlot::Widget("bump"),
        PointerButton::Primary,
        false,
        false,
    );
    assert_eq!(dispatches(&game), before, "latching is per-frame pure");

    game.server.tick_menu(0, &mut Default::default());
    assert_eq!(
        dispatches(&game),
        before + 1,
        "the tick dispatched gui_click to the owning mod"
    );

    // A secondary click over a button is consumed but triggers nothing.
    game.menu_click(
        crate::gui::MenuSlot::Widget("bump"),
        PointerButton::Secondary,
        false,
        false,
    );
    game.server.tick_menu(0, &mut Default::default());
    assert_eq!(dispatches(&game), before + 1);

    // Closing the session clears the map and drops the target. The close
    // message latches and applies on the tick (like play).
    crate::gui::gui_state_set(
        &mut game.server.sessions[0].gui_state,
        "modtest:mid".into(),
        GuiValue::F32(0.5),
    );
    game.close_open_menu();
    game.apply_latched_actions_for_test();
    assert!(game.server.sessions[0]
        .gui_state
        .get("modtest:mid")
        .is_none());

    // With no mod GUI session open, a stray widget click dispatches nothing.
    game.menu_click(
        crate::gui::MenuSlot::Widget("bump"),
        PointerButton::Primary,
        false,
        false,
    );
    game.server.tick_menu(0, &mut Default::default());
    assert_eq!(dispatches(&game), before + 1);
}

#[test]
fn chest_lids_follow_the_viewer_count_not_the_local_menu() {
    use crate::block::Block;
    use crate::mathh::IVec3;
    let mut game = game_on_empty_chunk();
    let pos = IVec3::new(8, 64, 8);
    game.server.world.set_block_world(8, 64, 8, Block::Chest);
    game.server
        .world
        .insert_chest(pos, crate::block_model::DEFAULT_MODEL_FACING);

    let mut ev = crate::game::TickEvents::default();
    game.server.open_chest_screen_for(0, pos, &mut ev);
    assert_eq!(game.server.chest_viewers.get(&pos), Some(&1));
    // Re-opening without a close never leaks a viewer slot.
    game.server.open_chest_screen_for(0, pos, &mut ev);
    assert_eq!(game.server.chest_viewers.get(&pos), Some(&1));

    // A second player looking inside keeps the lid up after the first leaves.
    *game.server.chest_viewers.entry(pos).or_insert(0) += 1;
    game.server.close_open_menu_for(0, &mut ev);
    assert_eq!(
        game.server.chest_viewers.get(&pos),
        Some(&1),
        "one viewer remains after the local player closes"
    );
    // The lid animation reads the REPLICATED open-chest set; mirror what the
    // next batch would ship.
    game.sync_open_chests_for_test();
    for _ in 0..30 {
        game.advance_chest_lids(0.05);
    }
    assert!(
        game.chest_lid_angle(pos) > 0.9,
        "the lid stays open while ANY player is looking inside"
    );

    // The last viewer leaving drops the lid.
    game.server.chest_viewers.remove(&pos);
    game.sync_open_chests_for_test();
    for _ in 0..60 {
        game.advance_chest_lids(0.05);
    }
    assert!(
        game.chest_lid_angle(pos) < 0.05,
        "the lid closes once the last viewer leaves"
    );
}
