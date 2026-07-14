use super::{app_with_grass, cursor_over_slot};
use crate::controls::{Control, Modifiers};

#[test]
fn drop_key_drops_one_held_item_while_playing() {
    let mut app = app_with_grass();
    let before = app.inventory().selected().unwrap().count;
    app.handle_control(Control::DropItem, true);
    assert_eq!(
        app.inventory().selected().unwrap().count,
        before,
        "drop is latched until the fixed tick applies it"
    );
    app.apply_latched_actions_for_test();
    assert_eq!(app.inventory().selected().unwrap().count, before - 1);
}

#[test]
fn sprint_plus_drop_drops_the_whole_held_stack() {
    // The whole-stack modifier is the SPRINT control (wherever it's bound),
    // per design — not the physical Ctrl modifier.
    let mut app = app_with_grass();
    assert!(app.inventory().selected().is_some());
    app.handle_control(Control::Sprint, true);
    app.handle_control(Control::DropItem, true);
    assert!(
        app.inventory().selected().is_some(),
        "drop-all is latched until the fixed tick applies it"
    );
    app.apply_latched_actions_for_test();
    assert!(app.inventory().selected().is_none(), "whole stack dropped");
}

#[test]
fn physical_ctrl_without_the_sprint_control_drops_one() {
    // Ctrl only counts when it IS the sprint binding (the default). Here only
    // the tracked physical modifier is set — the sprint control never fired —
    // so the drop stays a single item. Guards the coupling direction.
    let mut app = app_with_grass();
    app.set_modifiers(Modifiers {
        ctrl: true,
        shift: false,
        ..Modifiers::default()
    });
    let before = app.inventory().selected().unwrap().count;
    app.handle_control(Control::DropItem, true);
    app.apply_latched_actions_for_test();
    assert_eq!(
        app.inventory().selected().unwrap().count,
        before - 1,
        "the sprint CONTROL decides, not the raw modifier"
    );
}

#[test]
fn drop_key_drops_one_from_the_hovered_hotbar_slot_in_a_menu() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);
    let (x, y) = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(x, y);
    let before = app.inventory().slot(0).unwrap().count;
    app.handle_control(Control::DropItem, true);
    assert_eq!(
        app.inventory().slot(0).unwrap().count,
        before,
        "menu drop is latched to the fixed tick"
    );
    app.apply_latched_actions_for_test();
    assert_eq!(app.inventory().slot(0).unwrap().count, before - 1);
}

#[test]
fn ctrl_drop_drops_the_whole_hovered_main_inventory_stack() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let screen = (1280, 720);

    app.set_modifiers(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    let hotbar = cursor_over_slot(&mut app, screen, 0);
    app.set_cursor_position(hotbar.0, hotbar.1);
    app.click_screen_for_test(screen, 0.0);
    app.set_modifiers(Modifiers {
        ctrl: true,
        ..Modifiers::default()
    });
    assert!(app.inventory().slot(0).is_none());
    assert_eq!(app.inventory().slot(9).map(|stack| stack.count), Some(64));

    let main = cursor_over_slot(&mut app, screen, 9);
    app.set_cursor_position(main.0, main.1);
    app.handle_control(Control::DropItem, true);
    app.apply_latched_actions_for_test();
    assert!(app.inventory().slot(9).is_none());
}
