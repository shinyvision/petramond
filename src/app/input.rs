use crate::controls::Control;
use crate::game::MovementInput;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    ToggleInventory,
    OpenChat,
    TogglePlayerMode,
    CloseScreen,
    SelectHotbar(u8),
    /// Drop the held item this press (edge-triggered). Whether it's one item or
    /// the whole stack is decided by the App from the physical Ctrl modifier, not
    /// here — keeping the drop key independent of the sprint binding.
    DropItem,
    RotateHeldBlock,
    TogglePerspective,
}

#[derive(Default)]
pub struct InputController {
    forward: bool,
    backward: bool,
    left: bool,
    right: bool,
    jump: bool,
    sneak: bool,
    sprint: bool,
    toggle_mode_key: bool,
    toggle_mode_chord: bool,
    inventory_toggle_held: bool,
    chat_open_held: bool,
    drop_item_held: bool,
    rotate_held_block_held: bool,
    toggle_perspective_held: bool,
}

impl InputController {
    pub fn set_control(&mut self, control: Control, down: bool) -> Option<ControlEvent> {
        let event = match control {
            Control::MoveForward => {
                self.forward = down;
                None
            }
            Control::MoveBackward => {
                self.backward = down;
                None
            }
            Control::MoveLeft => {
                self.left = down;
                None
            }
            Control::MoveRight => {
                self.right = down;
                None
            }
            Control::Jump => {
                self.jump = down;
                None
            }
            Control::Sneak => {
                self.sneak = down;
                None
            }
            Control::Sprint => {
                self.sprint = down;
                None
            }
            Control::TogglePlayerMode => {
                self.toggle_mode_key = down;
                None
            }
            Control::ToggleInventory => {
                let edge = down && !self.inventory_toggle_held;
                self.inventory_toggle_held = down;
                edge.then_some(ControlEvent::ToggleInventory)
            }
            Control::OpenChat => {
                let edge = down && !self.chat_open_held;
                self.chat_open_held = down;
                edge.then_some(ControlEvent::OpenChat)
            }
            Control::CloseScreen => down.then_some(ControlEvent::CloseScreen),
            Control::SelectHotbar(slot) => down.then_some(ControlEvent::SelectHotbar(slot)),
            Control::DropItem => {
                // Edge-triggered: one drop per press. The whole-stack vs single
                // choice is the App's, read from the physical Ctrl modifier.
                let edge = down && !self.drop_item_held;
                self.drop_item_held = down;
                edge.then_some(ControlEvent::DropItem)
            }
            Control::RotateHeldBlock => {
                let edge = down && !self.rotate_held_block_held;
                self.rotate_held_block_held = down;
                edge.then_some(ControlEvent::RotateHeldBlock)
            }
            Control::TogglePerspective => {
                let edge = down && !self.toggle_perspective_held;
                self.toggle_perspective_held = down;
                edge.then_some(ControlEvent::TogglePerspective)
            }
        };

        event.or_else(|| self.mode_chord_event())
    }

    pub fn movement(&self) -> MovementInput {
        MovementInput {
            forward: self.forward,
            backward: self.backward,
            left: self.left,
            right: self.right,
            jump: self.jump,
            sneak: self.sneak,
            sprint: self.sprint,
        }
    }

    fn mode_chord_event(&mut self) -> Option<ControlEvent> {
        let chord = self.sprint && self.toggle_mode_key;
        let event = chord && !self.toggle_mode_chord;
        self.toggle_mode_chord = chord;
        event.then_some(ControlEvent::TogglePlayerMode)
    }
}

#[cfg(test)]
mod tests {
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
        // Next press fires again. Whole-stack vs single (the Ctrl modifier) is the
        // App's concern, no longer encoded in the event.
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
}
