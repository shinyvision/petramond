use super::{app, app_with_grass, cursor_over_craft, cursor_over_slot, panel_gap_point};
use crate::controls::{Control, Modifiers};
use crate::gui::{CraftHit, GuiKind};
use crate::item::{ItemStack, ItemType};

#[test]
fn craft_slot_clicks_route_through_to_crafting() {
    let mut app = app();
    // Give the player one oak log and open the inventory (2x2 crafting).
    app.game_mut()
        .add_to_inventory(ItemStack::new(ItemType::OakLog, 1));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);

    // Pick the log up from inventory slot 0.
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.game().inventory().cursor().is_some());

    // Drop it into the first 2x2 craft input cell -> planks preview appears.
    let cc = cursor_over_craft(screen, GuiKind::Inventory, CraftHit::Input(0));
    app.set_cursor_position(cc.0, cc.1);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.game().inventory().cursor().is_none(),
        "log placed into the craft cell"
    );
    assert_eq!(
        app.game().menu_read_model().craft.result().map(|s| s.item),
        Some(ItemType::OakPlanks)
    );

    // Click the result slot: 4 planks land on the cursor, ingredients consumed.
    let rc = cursor_over_craft(screen, GuiKind::Inventory, CraftHit::Result);
    app.set_cursor_position(rc.0, rc.1);
    app.click_screen_for_test(screen, 0.2);
    assert_eq!(
        app.game().inventory().cursor().map(|s| (s.item, s.count)),
        Some((ItemType::OakPlanks, 4))
    );
    assert!(app.game().menu_read_model().craft.result().is_none());
}

#[test]
fn closing_a_menu_returns_craft_grid_items_to_inventory() {
    let mut app = app();
    app.game_mut()
        .add_to_inventory(ItemStack::new(ItemType::OakLog, 2));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    // Move the logs onto the cursor and into a craft cell.
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    let cc = cursor_over_craft(
        screen,
        crate::gui::GuiKind::Inventory,
        crate::gui::CraftHit::Input(0),
    );
    app.set_cursor_position(cc.0, cc.1);
    app.click_screen_for_test(screen, 0.1);
    // Close with Escape: the logs return to the inventory.
    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
    let logs: u32 = (0..crate::inventory::TOTAL_SLOTS)
        .filter_map(|i| app.game().inventory().slot(i))
        .filter(|s| s.item == ItemType::OakLog)
        .map(|s| s.count as u32)
        .sum();
    assert_eq!(logs, 2, "craft-grid logs came back to the inventory");
}

#[test]
fn closing_a_menu_stashes_the_cursor_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.game().inventory().cursor().is_some());

    assert!(app.handle_control(Control::CloseScreen, true));

    assert!(app.game().inventory().cursor().is_none());
    let grass: u32 = (0..crate::inventory::TOTAL_SLOTS)
        .filter_map(|i| app.game().inventory().slot(i))
        .filter(|s| s.item == ItemType::Grass)
        .map(|s| s.count as u32)
        .sum();
    assert_eq!(grass, 64, "cursor stack was parked back in inventory");
}

#[test]
fn route_inventory_click_open_picks_up_slot_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);

    assert!(app.game().inventory().cursor().is_none());
    let item0 = app.game().inventory().slot(0).unwrap().item;

    let consumed = app.click_screen_for_test(screen, 0.0);
    assert!(consumed);
    assert!(app.game().inventory().slot(0).is_none());
    assert_eq!(app.game().inventory().cursor().unwrap().item, item0);
}

#[test]
fn fast_double_click_keeps_stack_on_cursor_to_gather() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);

    // First click picks the stack up; a second click within the double-click
    // window gathers matching items instead of dropping it back - so the stack
    // stays on the cursor and the source slot stays empty.
    app.click_screen_for_test(screen, 0.0);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.game().inventory().cursor().is_some(),
        "stack stays on the cursor"
    );
    assert!(
        app.game().inventory().slot(0).is_none(),
        "source slot stays empty"
    );
}

#[test]
fn slow_second_click_drops_the_stack_back() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);

    // Two clicks spaced beyond the double-click window: the second is a normal
    // click that drops the held stack back into the now-empty slot.
    app.click_screen_for_test(screen, 0.0);
    app.click_screen_for_test(screen, 1.0);
    assert!(
        app.game().inventory().cursor().is_none(),
        "stack dropped back"
    );
    assert!(app.game().inventory().slot(0).is_some(), "slot refilled");
}

#[test]
fn fast_click_on_a_different_slot_is_not_a_double_click() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    // Pick up slot 0's stack.
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.game().inventory().cursor().is_some());

    // A fast click on a DIFFERENT slot is a normal drop, not a gather: the held
    // stack lands in the first (empty) main-grid slot.
    let dest = crate::inventory::HOTBAR_LEN;
    let (dx, dy) = cursor_over_slot(screen, dest);
    app.set_cursor_position(dx, dy);
    app.click_screen_for_test(screen, 0.05);
    assert!(
        app.game().inventory().cursor().is_none(),
        "stack dropped into the new slot"
    );
    assert!(app.game().inventory().slot(dest).is_some());
}

#[test]
fn route_inventory_click_closed_is_a_noop() {
    let mut app = app();
    assert!(!app.screen.inventory_open());
    let before = app.game().inventory().slot(0).map(|s| s.count);
    let consumed = app.click_screen_for_test((1280, 720), 0.0);
    assert!(!consumed);
    assert!(app.game().inventory().cursor().is_none());
    assert_eq!(app.game().inventory().slot(0).map(|s| s.count), before);
}

#[test]
fn route_inventory_right_click_splits_slot_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    // Slot 0 starts at 64; right-click drags off the larger half (32).
    let consumed = app.right_click_screen_for_test(screen, 0.0);
    assert!(consumed);
    assert_eq!(app.game().inventory().cursor().unwrap().count, 32);
    assert_eq!(app.game().inventory().slot(0).unwrap().count, 32);
}

#[test]
fn route_inventory_right_click_closed_falls_through_to_placement() {
    // Closed inventory: a right-click is NOT consumed, so it can place a block.
    let mut app = app();
    assert!(!app.screen.inventory_open());
    assert!(!app.right_click_screen_for_test((1280, 720), 0.0));
}

#[test]
fn route_inventory_shift_click_moves_hotbar_to_main_grid() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    // Physical Shift modifier held (NOT via the sneak control).
    app.set_modifiers(Modifiers {
        ctrl: false,
        shift: true,
    });
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    let item0 = app.game().inventory().slot(0).unwrap().item;
    app.click_screen_for_test(screen, 0.0);
    assert!(
        app.game().inventory().slot(0).is_none(),
        "hotbar slot emptied"
    );
    assert_eq!(
        app.game()
            .inventory()
            .slot(crate::inventory::HOTBAR_LEN)
            .unwrap()
            .item,
        item0,
        "moved to the first main-grid slot"
    );
}

#[test]
fn route_click_outside_panel_throws_held_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    // Drag slot 0's stack onto the cursor.
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.game().inventory().cursor().is_some());
    // Click the top-left corner: confidently outside the inventory panel.
    app.set_cursor_position(0.0, 0.0);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.game().inventory().cursor().is_none(),
        "held stack thrown out of the inventory"
    );
}

#[test]
fn route_click_on_panel_background_does_not_throw() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0); // pick up the stack
    assert!(app.game().inventory().cursor().is_some());
    // A point inside the panel but on no slot: the held stack is kept.
    let inside_panel_gap = panel_gap_point(screen);
    app.set_cursor_position(inside_panel_gap.0, inside_panel_gap.1);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.game().inventory().cursor().is_some(),
        "click on panel art must not throw the stack"
    );
}
