use super::common::{apply_drop_actions, filled_inventory, game};
use crate::gui::MenuSlot;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};

#[test]
fn container_edits_apply_on_the_tick_not_the_frame() {
    let mut game = game();
    game.player.inventory = filled_inventory(); // a stack of Dirt in hotbar slot 0

    // Left-click that slot: it should pick the stack onto the cursor — but that's a
    // container edit, so it's latched, not applied this frame.
    game.menu_click(
        MenuSlot::Inventory(0),
        crate::controls::PointerButton::Primary,
        false,
        false,
    );
    assert!(
        game.player.inventory.cursor().is_none(),
        "the click hasn't applied yet — no cursor pickup this frame"
    );

    // The tick applies it, moving the stack onto the cursor.
    game.tick_menu(&mut Default::default());
    assert!(
        game.player.inventory.cursor().is_some(),
        "the tick applies the container edit (the stack is now on the cursor)"
    );
}

#[test]
fn cursor_has_stack_tracks_the_held_stack() {
    let mut game = game();
    game.player.inventory = filled_inventory();
    assert!(!game.cursor_has_stack(), "nothing held initially");
    game.player.inventory.click_slot(0); // pick up hotbar slot 0
    assert!(game.cursor_has_stack(), "holding a stack after pickup");
}

#[test]
fn closing_cursor_stack_uses_empty_inventory_slot_after_matching_stacks() {
    let mut game = game();
    let mut slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
    slots[4] = None;
    game.player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

    game.close_cursor_stack();

    assert!(game.player.inventory.cursor().is_none());
    assert_eq!(
        game.player.inventory.slot(4),
        Some(&ItemStack::new(ItemType::Dirt, 12))
    );
    apply_drop_actions(&mut game);
    assert!(
        game.world.item_entities().is_empty(),
        "stashed cursor stack should not drop"
    );
}

#[test]
fn closing_cursor_stack_queues_a_drop_when_inventory_is_full() {
    let mut game = game();
    let slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
    game.player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

    game.close_cursor_stack();

    assert!(game.player.inventory.cursor().is_none());
    assert!(
        game.world.item_entities().is_empty(),
        "drop waits for the next tick"
    );
    apply_drop_actions(&mut game);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(
        game.world.item_entities()[0].stack,
        ItemStack::new(ItemType::Dirt, 12)
    );
}

#[test]
fn closing_cursor_stack_fills_matching_partials_then_drops_leftover() {
    let mut game = game();
    let mut slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
    slots[2] = Some(ItemStack::new(ItemType::Dirt, 60));
    slots[10] = Some(ItemStack::new(ItemType::Dirt, 63));
    game.player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

    game.close_cursor_stack();

    assert!(game.player.inventory.cursor().is_none());
    assert_eq!(
        game.player.inventory.slot(2),
        Some(&ItemStack::new(ItemType::Dirt, 64))
    );
    assert_eq!(
        game.player.inventory.slot(10),
        Some(&ItemStack::new(ItemType::Dirt, 64))
    );
    assert!(
        game.world.item_entities().is_empty(),
        "leftover drop waits for the next tick"
    );
    apply_drop_actions(&mut game);
    assert_eq!(game.world.item_entities().len(), 1);
    assert_eq!(
        game.world.item_entities()[0].stack,
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
    game.player.inventory =
        Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 5)), 0);

    game.collect_to_cursor();

    // 5 + 20 + 30 = 55 onto the cursor, both dirt sources emptied.
    assert_eq!(game.player.inventory.cursor().unwrap().count, 55);
    assert!(game.player.inventory.slot(2).is_none());
    assert!(game
        .player
        .inventory
        .slot(crate::inventory::HOTBAR_LEN)
        .is_none());
    assert_eq!(game.player.inventory.slot(5).unwrap().item, ItemType::Stone);
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
    game.world
        .gui_state_set("modtest:stale".into(), GuiValue::I32(9));
    game.open_mod_gui_screen(kind, Some(crate::mathh::IVec3::new(1, 2, 3)));
    assert!(
        game.world.gui_state_get("modtest:stale").is_none(),
        "opening a mod GUI clears the session state map"
    );

    let dispatches = |game: &super::super::Game| game.mods_for_test().probe(0).1;
    let before = dispatches(&game);

    // The click latches this frame; nothing dispatches until the tick.
    game.menu_click(
        crate::gui::MenuSlot::Widget("bump"),
        PointerButton::Primary,
        false,
        false,
    );
    assert_eq!(dispatches(&game), before, "latching is per-frame pure");

    game.tick_menu(&mut Default::default());
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
    game.tick_menu(&mut Default::default());
    assert_eq!(dispatches(&game), before + 1);

    // Closing the session clears the map and drops the target.
    game.world
        .gui_state_set("modtest:mid".into(), GuiValue::F32(0.5));
    game.close_open_menu();
    assert!(game.world.gui_state_get("modtest:mid").is_none());

    // With no mod GUI session open, a stray widget click dispatches nothing.
    game.menu_click(
        crate::gui::MenuSlot::Widget("bump"),
        PointerButton::Primary,
        false,
        false,
    );
    game.tick_menu(&mut Default::default());
    assert_eq!(dispatches(&game), before + 1);
}
