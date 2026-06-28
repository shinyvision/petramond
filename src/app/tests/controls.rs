use super::app;
use crate::controls::Control;
use crate::player::PlayerMode;

#[test]
fn ctrl_y_toggles_player_mode_once_per_chord() {
    let mut app = app();
    assert_eq!(app.game.player_mode(), PlayerMode::Survival);

    app.handle_control(Control::Sprint, true);
    app.handle_control(Control::TogglePlayerMode, true);
    assert_eq!(app.game.player_mode(), PlayerMode::Spectator);

    app.handle_control(Control::TogglePlayerMode, true);
    app.handle_control(Control::Sprint, true);
    assert_eq!(app.game.player_mode(), PlayerMode::Spectator);

    app.handle_control(Control::TogglePlayerMode, false);
    app.handle_control(Control::TogglePlayerMode, true);
    assert_eq!(app.game.player_mode(), PlayerMode::Survival);

    app.handle_control(Control::Sprint, false);
    app.handle_control(Control::TogglePlayerMode, false);
    app.handle_control(Control::TogglePlayerMode, true);
    assert_eq!(app.game.player_mode(), PlayerMode::Survival);
}

#[test]
fn inventory_toggle_is_once_per_press() {
    let mut app = app();
    assert!(!app.screen.inventory_open());

    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());

    app.handle_control(Control::ToggleInventory, false);
    app.handle_control(Control::ToggleInventory, true);
    assert!(!app.screen.inventory_open());
}

#[test]
fn opening_inventory_releases_grab() {
    let mut app = app();
    app.pointer.grab_for_gameplay();
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    assert!(!app.pointer.is_grabbing());
}

#[test]
fn escape_closes_open_inventory_and_regrabs() {
    let mut app = app();
    app.handle_control(Control::ToggleInventory, true);
    assert!(app.screen.inventory_open());
    assert!(!app.pointer.is_grabbing());

    assert!(app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
    assert!(app.pointer.is_grabbing());
}

#[test]
fn escape_with_inventory_closed_is_not_consumed() {
    let mut app = app();
    assert!(!app.screen.inventory_open());
    assert!(!app.handle_control(Control::CloseScreen, true));
    assert!(!app.screen.inventory_open());
}

#[test]
fn digit_controls_select_hotbar_slot() {
    let mut app = app();
    app.handle_control(Control::SelectHotbar(4), true);
    assert_eq!(app.game.inventory().active_slot(), 4);
    app.handle_control(Control::SelectHotbar(0), true);
    assert_eq!(app.game.inventory().active_slot(), 0);
    app.handle_control(Control::SelectHotbar(8), true);
    assert_eq!(app.game.inventory().active_slot(), 8);
}

#[test]
fn digit_controls_ignored_while_inventory_open() {
    let mut app = app();
    app.handle_control(Control::SelectHotbar(2), true);
    assert_eq!(app.game.inventory().active_slot(), 2);
    app.handle_control(Control::ToggleInventory, true);
    app.handle_control(Control::SelectHotbar(6), true);
    assert_eq!(app.game.inventory().active_slot(), 2);
}
