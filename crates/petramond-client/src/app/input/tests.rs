use super::*;

#[test]
fn inventory_toggle_is_edge_triggered() {
    let mut input = InputController::default();
    assert_eq!(
        input.set_control(Control::ToggleInventory, true),
        Some(ControlEvent::ToggleInventory)
    );
    assert_eq!(input.set_control(Control::ToggleInventory, true), None);
    assert_eq!(input.set_control(Control::ToggleInventory, false), None);
    assert_eq!(
        input.set_control(Control::ToggleInventory, true),
        Some(ControlEvent::ToggleInventory)
    );
}

#[test]
fn drop_item_is_edge_triggered() {
    let mut input = InputController::default();
    // One event per press.
    assert_eq!(
        input.set_control(Control::DropItem, true),
        Some(ControlEvent::DropItem)
    );
    // Holding Q does not repeat the drop.
    assert_eq!(input.set_control(Control::DropItem, true), None);
    assert_eq!(input.set_control(Control::DropItem, false), None);
    // Next press fires again. Whole-stack vs single is contextual App
    // policy, no longer encoded in the event.
    assert_eq!(
        input.set_control(Control::DropItem, true),
        Some(ControlEvent::DropItem)
    );
}

#[test]
fn rotate_held_block_is_edge_triggered() {
    let mut input = InputController::default();
    assert_eq!(
        input.set_control(Control::RotateHeldBlock, true),
        Some(ControlEvent::RotateHeldBlock)
    );
    assert_eq!(input.set_control(Control::RotateHeldBlock, true), None);
    assert_eq!(input.set_control(Control::RotateHeldBlock, false), None);
    assert_eq!(
        input.set_control(Control::RotateHeldBlock, true),
        Some(ControlEvent::RotateHeldBlock)
    );
}

#[test]
fn ctrl_y_chord_is_edge_triggered_from_either_order() {
    let mut input = InputController::default();
    assert_eq!(input.set_control(Control::Sprint, true), None);
    assert_eq!(
        input.set_control(Control::TogglePlayerMode, true),
        Some(ControlEvent::TogglePlayerMode)
    );
    assert_eq!(input.set_control(Control::TogglePlayerMode, true), None);
    assert_eq!(input.set_control(Control::TogglePlayerMode, false), None);
    assert_eq!(
        input.set_control(Control::TogglePlayerMode, true),
        Some(ControlEvent::TogglePlayerMode)
    );

    let mut input = InputController::default();
    assert_eq!(input.set_control(Control::TogglePlayerMode, true), None);
    assert_eq!(
        input.set_control(Control::Sprint, true),
        Some(ControlEvent::TogglePlayerMode)
    );
}
