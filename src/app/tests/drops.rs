use super::app_with_grass;
use crate::controls::{Control, Modifiers};

#[test]
fn q_drops_one_held_item_while_playing() {
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
fn ctrl_q_drops_whole_held_stack_while_playing() {
    let mut app = app_with_grass();
    assert!(app.inventory().selected().is_some());
    // Physical Ctrl modifier held (NOT via the sprint control).
    app.set_modifiers(Modifiers {
        ctrl: true,
        shift: false,
    });
    app.handle_control(Control::DropItem, true);
    assert!(
        app.inventory().selected().is_some(),
        "drop-all is latched until the fixed tick applies it"
    );
    app.apply_latched_actions_for_test();
    assert!(app.inventory().selected().is_none(), "whole stack dropped");
}

#[test]
fn q_drops_one_even_while_sprinting_when_ctrl_not_tracked() {
    // Holding the sprint *control* must NOT turn Q into a drop-all: only the
    // physical Ctrl modifier does. Guards the decoupling from the keybind.
    let mut app = app_with_grass();
    app.handle_control(Control::Sprint, true);
    let before = app.inventory().selected().unwrap().count;
    app.handle_control(Control::DropItem, true);
    assert_eq!(
        app.inventory().selected().unwrap().count,
        before,
        "drop is latched until the fixed tick applies it"
    );
    app.apply_latched_actions_for_test();
    assert_eq!(
        app.inventory().selected().unwrap().count,
        before - 1,
        "sprint key alone drops one, not the whole stack"
    );
}

#[test]
fn q_does_not_drop_while_inventory_open() {
    let mut app = app_with_grass();
    app.handle_control(Control::ToggleInventory, true);
    let before = app.inventory().selected().map(|s| s.count);
    app.handle_control(Control::DropItem, true);
    assert_eq!(app.inventory().selected().map(|s| s.count), before);
}
