use super::{
    app, app_with_grass, cursor_over_craft_result, cursor_over_menu, cursor_over_slot,
    cursor_over_widget, panel_gap_point,
};
use crate::controls::{Control, Modifiers};
use crate::item::{ItemStack, ItemType};

fn search_recipes(app: &mut super::TestApp, screen: (u32, u32), query: &str) {
    let (x, y) = cursor_over_widget(app, screen, "craft_search", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.handle_text_input(query));
    // The first frame resolves TextChanged; the second repopulates the real
    // list document from the browser's new query.
    app.solve_menu_frame_for_test(screen);
    app.solve_menu_frame_for_test(screen);
}

fn replace_recipe_search(app: &mut super::TestApp, screen: (u32, u32), query: &str) {
    let (x, y) = cursor_over_widget(app, screen, "craft_search", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.0);
    app.set_modifiers(Modifiers {
        ctrl: true,
        ..Modifiers::default()
    });
    assert!(app.handle_text_shortcut_code(winit::keyboard::KeyCode::KeyA));
    app.set_modifiers(Modifiers::default());
    assert!(app.handle_text_input(query));
    app.solve_menu_frame_for_test(screen);
    app.solve_menu_frame_for_test(screen);
}

fn select_first_recipe_and_craft(app: &mut super::TestApp, screen: (u32, u32)) {
    let (x, y) = cursor_over_widget(app, screen, "recipe", Some(0));
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.1);
    let (x, y) = cursor_over_widget(app, screen, "craft", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.2);
}

#[test]
fn recipe_browser_searches_selects_crafts_and_routes_output_take() {
    let mut app = app();
    app.install_test_crafting_recipe();
    app.add_to_inventory(ItemStack::new(ItemType::Coal, 1));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);

    search_recipes(&mut app, screen, "stick");
    let query = app
        .ui
        .state_mut()
        .get_str("craft_search")
        .map(str::to_owned);
    let rows = app.ui.state_mut().get_list("craft_recipes").cloned();
    assert!(
        rows.as_ref().is_some_and(|rows| rows.len() == 1),
        "the real inventory document was filtered to the matching recipe; query={query:?}, rows={rows:?}"
    );
    select_first_recipe_and_craft(&mut app, screen);
    assert_eq!(
        app.game().menu_read_model().craft_output.map(|s| s.item),
        Some(ItemType::Stick)
    );

    let rc = cursor_over_craft_result(&mut app, screen);
    app.set_cursor_position(rc.0, rc.1);
    app.click_screen_for_test(screen, 0.3);
    assert_eq!(
        app.inventory().cursor().map(|s| s.item),
        Some(ItemType::Stick)
    );
    assert!(app.game().menu_read_model().craft_output.is_none());
}

#[test]
fn unaffordable_recipe_row_stays_disabled_and_cannot_be_selected() {
    let mut app = app();
    app.install_test_crafting_recipe();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);

    search_recipes(&mut app, screen, "stick");
    let rows = app
        .ui
        .state_mut()
        .get_list("craft_recipes")
        .expect("matching row exists");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("enabled"),
        Some(&petramond_ui::UiValue::Bool(false))
    );

    let (x, y) = cursor_over_widget(&mut app, screen, "recipe", Some(0));
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.1);
    app.solve_menu_frame_for_test(screen);
    assert_eq!(app.ui.state_mut().get_i32("craft_recipe_sel"), Some(-1));
    assert_eq!(app.ui.state_mut().get_bool("can_craft"), Some(false));
    assert!(app.game().menu_read_model().craft_output.is_none());
}

#[test]
fn crafting_search_owns_key_presses_but_not_releases_or_escape() {
    use winit::keyboard::KeyCode;

    let mut app = app();
    assert!(app.handle_raw_key(KeyCode::KeyW, true));
    assert!(app.take_game_input().movement.forward);
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    let (x, y) = cursor_over_widget(&mut app, screen, "craft_search", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.ui.text_input_focused());

    assert!(app.handle_raw_key(KeyCode::KeyW, false));
    assert!(
        !app.take_game_input().movement.forward,
        "a movement key held before focus must still release"
    );
    assert!(app.handle_raw_key(KeyCode::KeyE, true));
    assert!(
        app.screen.inventory_open(),
        "typing E must not toggle the menu"
    );
    assert!(app.handle_text_input("e"));
    app.solve_menu_frame_for_test(screen);
    app.solve_menu_frame_for_test(screen);
    assert_eq!(app.ui.state_mut().get_str("craft_search"), Some("e"));
    assert!(app.handle_raw_key(KeyCode::KeyE, false));
    assert!(app.screen.inventory_open());

    assert!(app.handle_raw_key(KeyCode::Escape, true));
    assert!(!app.screen.inventory_open(), "Escape still closes the menu");
}

#[test]
fn recipe_rows_are_cached_until_the_search_or_inventory_changes() {
    let mut app = app();
    app.install_test_crafting_recipe();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    app.solve_menu_frame_for_test(screen);
    let first = app
        .ui
        .state_mut()
        .get_list("craft_recipes")
        .expect("recipe rows")
        .clone();

    app.solve_menu_frame_for_test(screen);
    let unchanged = app
        .ui
        .state_mut()
        .get_list("craft_recipes")
        .expect("recipe rows")
        .clone();
    assert!(
        std::sync::Arc::ptr_eq(&first, &unchanged),
        "static rows and affordability are not rebuilt every frame"
    );

    app.add_to_inventory(ItemStack::new(ItemType::Coal, 1));
    app.solve_menu_frame_for_test(screen);
    let refreshed = app
        .ui
        .state_mut()
        .get_list("craft_recipes")
        .expect("recipe rows")
        .clone();
    assert!(!std::sync::Arc::ptr_eq(&first, &refreshed));
}

#[test]
fn hidden_recipe_selection_returns_when_the_search_matches_again() {
    let mut app = app();
    app.install_test_crafting_recipe();
    app.add_to_inventory(ItemStack::new(ItemType::Coal, 1));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    search_recipes(&mut app, screen, "stick");
    let (x, y) = cursor_over_widget(&mut app, screen, "recipe", Some(0));
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.1);
    app.solve_menu_frame_for_test(screen);
    assert_eq!(app.ui.state_mut().get_i32("craft_recipe_sel"), Some(0));

    replace_recipe_search(&mut app, screen, "no match");
    assert_eq!(app.ui.state_mut().get_i32("craft_recipe_sel"), Some(-1));
    replace_recipe_search(&mut app, screen, "stick");
    assert_eq!(
        app.ui.state_mut().get_i32("craft_recipe_sel"),
        Some(0),
        "selection is stable-key state, not a disposable filtered index"
    );
}

#[test]
fn crafting_buttons_do_not_use_the_shell_ui_click_sound() {
    let mut app = app();
    app.install_test_crafting_recipe();
    app.add_to_inventory(ItemStack::new(ItemType::Coal, 1));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    search_recipes(&mut app, screen, "stick");
    app.audio.take_played_for_test();

    let (x, y) = cursor_over_widget(&mut app, screen, "recipe", Some(0));
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.1);
    let (x, y) = cursor_over_widget(&mut app, screen, "craft", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.2);

    assert!(app.audio.take_played_for_test().is_empty());
}

#[test]
fn closing_a_menu_stashes_untaken_craft_output() {
    let mut app = app();
    app.install_test_crafting_recipe();
    app.add_to_inventory(ItemStack::new(ItemType::Coal, 1));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    search_recipes(&mut app, screen, "stick");
    select_first_recipe_and_craft(&mut app, screen);
    assert!(app.game().menu_read_model().craft_output.is_some());

    assert!(app.handle_control(Control::CloseScreen, true));
    app.apply_latched_actions_for_test();
    assert!(!app.screen.inventory_open());
    let sticks: u32 = (0..crate::inventory::TOTAL_SLOTS)
        .filter_map(|i| app.inventory().slot(i))
        .filter(|s| s.item == ItemType::Stick)
        .map(|s| s.count as u32)
        .sum();
    assert!(sticks > 0, "untaken output was parked in the inventory");
    assert!(app.game().menu_read_model().craft_output.is_none());
}

#[test]
fn closing_a_menu_stashes_the_cursor_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.inventory().cursor().is_some());

    assert!(app.handle_control(Control::CloseScreen, true));
    // The close is a latched message now; apply it as the next tick would.
    app.apply_latched_actions_for_test();

    assert!(app.inventory().cursor().is_none());
    let grass: u32 = (0..crate::inventory::TOTAL_SLOTS)
        .filter_map(|i| app.inventory().slot(i))
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
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);

    assert!(app.inventory().cursor().is_none());
    let item0 = app.inventory().slot(0).unwrap().item;

    let consumed = app.click_screen_for_test(screen, 0.0);
    assert!(consumed);
    assert!(app.inventory().slot(0).is_none());
    assert_eq!(app.inventory().cursor().unwrap().item, item0);
}

#[test]
fn fast_double_click_keeps_stack_on_cursor_to_gather() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);

    // First click picks the stack up; a second click within the double-click
    // window gathers matching items instead of dropping it back - so the stack
    // stays on the cursor and the source slot stays empty.
    app.click_screen_for_test(screen, 0.0);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.inventory().cursor().is_some(),
        "stack stays on the cursor"
    );
    assert!(app.inventory().slot(0).is_none(), "source slot stays empty");
}

#[test]
fn slow_second_click_drops_the_stack_back() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);

    // Two clicks spaced beyond the double-click window: the second is a normal
    // click that drops the held stack back into the now-empty slot.
    app.click_screen_for_test(screen, 0.0);
    app.click_screen_for_test(screen, 1.0);
    assert!(app.inventory().cursor().is_none(), "stack dropped back");
    assert!(app.inventory().slot(0).is_some(), "slot refilled");
}

#[test]
fn fast_click_on_a_different_slot_is_not_a_double_click() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    // Pick up slot 0's stack.
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.inventory().cursor().is_some());

    // A fast click on a DIFFERENT slot is a normal drop, not a gather: the held
    // stack lands in the first (empty) main-grid slot.
    let dest = crate::inventory::HOTBAR_LEN;
    let (dx, dy) = cursor_over_slot(&mut app, screen, dest);
    app.set_cursor_position(dx, dy);
    app.click_screen_for_test(screen, 0.05);
    assert!(
        app.inventory().cursor().is_none(),
        "stack dropped into the new slot"
    );
    assert!(app.inventory().slot(dest).is_some());
}

#[test]
fn route_inventory_click_closed_is_a_noop() {
    let mut app = app();
    assert!(!app.screen.inventory_open());
    let before = app.inventory().slot(0).map(|s| s.count);
    let consumed = app.click_screen_for_test((1280, 720), 0.0);
    assert!(!consumed);
    assert!(app.inventory().cursor().is_none());
    assert_eq!(app.inventory().slot(0).map(|s| s.count), before);
}

#[test]
fn route_inventory_right_click_splits_slot_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);
    // Slot 0 starts at 64; right-click drags off the larger half (32).
    let consumed = app.right_click_screen_for_test(screen, 0.0);
    assert!(consumed);
    assert_eq!(app.inventory().cursor().unwrap().count, 32);
    assert_eq!(app.inventory().slot(0).unwrap().count, 32);
}

#[test]
fn primary_drag_preview_redivides_slots_before_release() {
    let mut app = app();
    app.add_to_inventory(ItemStack::new(ItemType::Grass, 10));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let source = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(source.0, source.1);
    app.click_screen_for_test(screen, 0.0);

    let destinations = [
        crate::gui::MenuSlot::Inventory(9),
        crate::gui::MenuSlot::Inventory(10),
        crate::gui::MenuSlot::Inventory(11),
    ];
    let points: Vec<_> = destinations
        .iter()
        .map(|&slot| cursor_over_menu(&mut app, screen, slot))
        .collect();
    let kind = app.doc_ui_kind().expect("inventory document");

    app.set_cursor_position(points[0].0, points[0].1);
    app.set_pointer_button(crate::controls::PointerButton::Primary, true);
    app.drive_doc_menu(kind, screen, 0.1);
    let preview = app.menu_snapshot_for_test();
    assert_eq!(preview.slots[9], Some((ItemType::Grass, 10)));
    assert!(preview.cursor.is_none());

    app.set_cursor_position(points[1].0, points[1].1);
    app.drive_doc_menu(kind, screen, 0.2);
    let preview = app.menu_snapshot_for_test();
    assert_eq!(preview.slots[9], Some((ItemType::Grass, 5)));
    assert_eq!(preview.slots[10], Some((ItemType::Grass, 5)));

    app.set_cursor_position(points[0].0, points[0].1);
    app.drive_doc_menu(kind, screen, 0.3);
    let preview = app.menu_snapshot_for_test();
    assert_eq!(preview.slots[9], Some((ItemType::Grass, 5)));
    assert_eq!(preview.slots[10], Some((ItemType::Grass, 5)));

    app.set_cursor_position(points[2].0, points[2].1);
    app.drive_doc_menu(kind, screen, 0.4);
    let preview = app.menu_snapshot_for_test();
    assert_eq!(preview.slots[9], Some((ItemType::Grass, 3)));
    assert_eq!(preview.slots[10], Some((ItemType::Grass, 3)));
    assert_eq!(preview.slots[11], Some((ItemType::Grass, 4)));
    assert!(preview.cursor.is_none());

    assert_eq!(app.inventory().cursor().map(|stack| stack.count), Some(10));
    assert!(app.inventory().slot(9).is_none());
    assert!(app.inventory().slot(10).is_none());
    assert!(app.inventory().slot(11).is_none());

    app.set_pointer_button(crate::controls::PointerButton::Primary, false);
    app.drive_doc_menu(kind, screen, 0.5);
    let released = app.menu_snapshot_for_test();
    assert_eq!(released.slots[9], Some((ItemType::Grass, 3)));
    assert_eq!(released.slots[10], Some((ItemType::Grass, 3)));
    assert_eq!(released.slots[11], Some((ItemType::Grass, 4)));
    assert!(
        released.cursor.is_none(),
        "release keeps the predicted frame"
    );
    assert!(
        app.inventory().slot(9).is_none(),
        "the server has not applied the release yet"
    );
    app.apply_latched_actions_for_test();
    assert_eq!(app.inventory().slot(9).map(|stack| stack.count), Some(3));
    assert_eq!(app.inventory().slot(10).map(|stack| stack.count), Some(3));
    assert_eq!(app.inventory().slot(11).map(|stack| stack.count), Some(4));
}

#[test]
fn secondary_drag_preview_places_one_on_each_new_slot_before_release() {
    let mut app = app();
    app.add_to_inventory(ItemStack::new(ItemType::Grass, 5));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let source = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(source.0, source.1);
    app.click_screen_for_test(screen, 0.0);

    let first = cursor_over_slot(&mut app, screen, 9);
    let second = cursor_over_slot(&mut app, screen, 10);
    let kind = app.doc_ui_kind().expect("inventory document");
    app.set_cursor_position(first.0, first.1);
    app.set_pointer_button(crate::controls::PointerButton::Secondary, true);
    app.drive_doc_menu(kind, screen, 0.1);
    let preview = app.menu_snapshot_for_test();
    assert_eq!(preview.slots[9], Some((ItemType::Grass, 1)));
    assert_eq!(preview.cursor, Some((ItemType::Grass, 4)));

    app.set_cursor_position(second.0, second.1);
    app.drive_doc_menu(kind, screen, 0.2);
    app.set_cursor_position(first.0, first.1);
    app.drive_doc_menu(kind, screen, 0.3);
    let preview = app.menu_snapshot_for_test();
    assert_eq!(preview.slots[9], Some((ItemType::Grass, 1)));
    assert_eq!(preview.slots[10], Some((ItemType::Grass, 1)));
    assert_eq!(preview.cursor, Some((ItemType::Grass, 3)));

    assert_eq!(app.inventory().cursor().map(|stack| stack.count), Some(5));
    assert!(app.inventory().slot(9).is_none());
    assert!(app.inventory().slot(10).is_none());

    app.set_pointer_button(crate::controls::PointerButton::Secondary, false);
    app.drive_doc_menu(kind, screen, 0.4);
    let released = app.menu_snapshot_for_test();
    assert_eq!(released.slots[9], Some((ItemType::Grass, 1)));
    assert_eq!(released.slots[10], Some((ItemType::Grass, 1)));
    assert_eq!(released.cursor, Some((ItemType::Grass, 3)));
    app.apply_latched_actions_for_test();
    assert_eq!(app.inventory().slot(9).map(|stack| stack.count), Some(1));
    assert_eq!(app.inventory().slot(10).map(|stack| stack.count), Some(1));
    assert_eq!(app.inventory().cursor().map(|stack| stack.count), Some(3));
}

#[test]
fn primary_drag_evenly_splits_the_cursor_and_puts_remainder_last() {
    let mut app = app();
    app.add_to_inventory(ItemStack::new(ItemType::Grass, 10));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let source = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(source.0, source.1);
    app.click_screen_for_test(screen, 0.0);
    assert_eq!(app.inventory().cursor().map(|stack| stack.count), Some(10));

    app.drag_screen_for_test(
        screen,
        0.1,
        crate::controls::PointerButton::Primary,
        &[
            crate::gui::MenuSlot::Inventory(9),
            crate::gui::MenuSlot::Inventory(10),
            crate::gui::MenuSlot::Inventory(9),
            crate::gui::MenuSlot::Inventory(11),
        ],
    );

    assert!(app.inventory().cursor().is_none());
    assert_eq!(app.inventory().slot(9).map(|stack| stack.count), Some(3));
    assert_eq!(app.inventory().slot(10).map(|stack| stack.count), Some(3));
    assert_eq!(app.inventory().slot(11).map(|stack| stack.count), Some(4));
}

#[test]
fn secondary_drag_places_once_per_distinct_slot_per_press() {
    let mut app = app();
    app.add_to_inventory(ItemStack::new(ItemType::Grass, 5));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let source = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(source.0, source.1);
    app.click_screen_for_test(screen, 0.0);

    let destinations = [
        crate::gui::MenuSlot::Inventory(9),
        crate::gui::MenuSlot::Inventory(10),
        crate::gui::MenuSlot::Inventory(9),
        crate::gui::MenuSlot::Inventory(11),
    ];
    app.drag_screen_for_test(
        screen,
        0.1,
        crate::controls::PointerButton::Secondary,
        &destinations,
    );
    assert_eq!(app.inventory().cursor().map(|stack| stack.count), Some(2));
    for slot in 9..=11 {
        assert_eq!(app.inventory().slot(slot).map(|stack| stack.count), Some(1));
    }

    app.drag_screen_for_test(
        screen,
        0.2,
        crate::controls::PointerButton::Secondary,
        &destinations[..2],
    );
    assert!(app.inventory().cursor().is_none());
    assert_eq!(app.inventory().slot(9).map(|stack| stack.count), Some(2));
    assert_eq!(app.inventory().slot(10).map(|stack| stack.count), Some(2));
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
        ..Modifiers::default()
    });
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);
    let item0 = app.inventory().slot(0).unwrap().item;
    app.click_screen_for_test(screen, 0.0);
    assert!(app.inventory().slot(0).is_none(), "hotbar slot emptied");
    assert_eq!(
        app.inventory()
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
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0);
    assert!(app.inventory().cursor().is_some());
    // Click the top-left corner: confidently outside the inventory panel.
    app.set_cursor_position(0.0, 0.0);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.inventory().cursor().is_none(),
        "held stack thrown out of the inventory"
    );
}

#[test]
fn route_click_on_panel_background_does_not_throw() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (cx, cy) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(cx, cy);
    app.click_screen_for_test(screen, 0.0); // pick up the stack
    assert!(app.inventory().cursor().is_some());
    // A point inside the panel but on no slot: the held stack is kept.
    let inside_panel_gap = panel_gap_point(&mut app, screen);
    app.set_cursor_position(inside_panel_gap.0, inside_panel_gap.1);
    app.click_screen_for_test(screen, 0.1);
    assert!(
        app.inventory().cursor().is_some(),
        "click on panel art must not throw the stack"
    );
}

#[test]
fn craftable_recipes_sort_first_and_the_filter_hides_the_rest() {
    let mut app = app();
    // Catalog order deliberately puts the UNAFFORDABLE recipe first.
    app.install_test_crafting_catalog(vec![
        super::test_recipe(
            "test:planks",
            ItemType::OakPlanks,
            ItemStack::new(ItemType::OakPlanks, 1),
        ),
        super::test_recipe("test:sticks", ItemType::Coal, ItemStack::new(ItemType::Stick, 2)),
    ]);
    app.add_to_inventory(ItemStack::new(ItemType::Coal, 1));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);
    app.solve_menu_frame_for_test(screen);

    let rows = app
        .ui
        .state_mut()
        .get_list("craft_recipes")
        .expect("recipe rows")
        .clone();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("enabled"),
        Some(&petramond_ui::UiValue::Bool(true)),
        "the craftable recipe leads even though the catalog lists it second"
    );
    assert_eq!(
        rows[1].get("enabled"),
        Some(&petramond_ui::UiValue::Bool(false))
    );

    let (x, y) = cursor_over_widget(&mut app, screen, "craft_filter", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.1);
    app.solve_menu_frame_for_test(screen);
    let rows = app
        .ui
        .state_mut()
        .get_list("craft_recipes")
        .expect("recipe rows")
        .clone();
    assert_eq!(rows.len(), 1, "the filter hides uncraftable recipes");
    assert_eq!(app.ui.state_mut().get_bool("craft_filter_on"), Some(true));
    assert!(
        app.server.sessions[0].player.craft_craftable_only,
        "the toggle reaches the server player, whose save carries it"
    );
}

#[test]
fn stackable_output_keeps_craft_enabled_and_shift_crafts_the_maximum() {
    let mut app = app();
    app.install_test_crafting_recipe();
    let max = ItemType::Stick.max_stack_size();
    app.add_to_inventory(ItemStack::new(ItemType::Coal, max));
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280u32, 720u32);

    search_recipes(&mut app, screen, "stick");
    select_first_recipe_and_craft(&mut app, screen);
    assert_eq!(
        app.game().menu_read_model().craft_output,
        Some(ItemStack::new(ItemType::Stick, 2))
    );
    app.solve_menu_frame_for_test(screen);
    assert_eq!(
        app.ui.state_mut().get_bool("can_craft"),
        Some(true),
        "a same-item output must not disable CRAFT"
    );

    // A repeat click merges into the output stack.
    let (x, y) = cursor_over_widget(&mut app, screen, "craft", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.3);
    assert_eq!(
        app.game().menu_read_model().craft_output,
        Some(ItemStack::new(ItemType::Stick, 4))
    );

    // Shift+CRAFT fills the rest of the output stack in one request.
    app.set_modifiers(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    let (x, y) = cursor_over_widget(&mut app, screen, "craft", None);
    app.set_cursor_position(x, y);
    app.click_screen_for_test(screen, 0.4);
    app.set_modifiers(Modifiers::default());
    assert_eq!(
        app.game().menu_read_model().craft_output,
        Some(ItemStack::new(ItemType::Stick, max / 2 * 2))
    );
    app.solve_menu_frame_for_test(screen);
    assert_eq!(
        app.ui.state_mut().get_bool("can_craft"),
        Some(false),
        "a FULL output stack disables CRAFT until it is taken"
    );
}
