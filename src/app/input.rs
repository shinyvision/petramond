use crate::controls::Control;
use crate::game::MovementInput;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    ToggleInventory,
    TogglePlayerMode,
    CloseScreen,
    SelectHotbar(u8),
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
            Control::CloseScreen => down.then_some(ControlEvent::CloseScreen),
            Control::SelectHotbar(slot) => down.then_some(ControlEvent::SelectHotbar(slot)),
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
