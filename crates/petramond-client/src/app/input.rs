use crate::app::pointer::PointerState;
use crate::game::MovementInput;
use petramond_input::controls::{ActionTable, BindingEngine, Control, Modifiers};

pub(super) struct Controls {
    pub(super) input: InputController,
    pub(super) pointer: PointerState,
    pub(super) modifiers: Modifiers,
    pub(super) action_table: ActionTable,
    pub(super) binding_engine: BindingEngine,
}

impl Controls {
    pub(super) fn new() -> Self {
        Self {
            input: InputController::default(),
            pointer: PointerState::default(),
            modifiers: Modifiers::default(),
            action_table: ActionTable::engine(),
            binding_engine: BindingEngine::default(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    ToggleInventory,
    OpenChat { command: bool },
    TogglePlayerMode,
    ToggleCreative,
    UndoEdit,
    RedoEdit,
    AdjustTool(i32),
    JumpPressed,
    CloseScreen,
    SelectHotbar(u8),
    Attack { down: bool },
    Interact { down: bool },
    DropItem,
    SwapOffHand,
    RotateHeldBlock,
    TogglePerspective,
}

#[derive(Default)]
struct HeldControls(Vec<Control>);

impl HeldControls {
    fn set(&mut self, control: Control, down: bool) -> bool {
        let held = self.is_held(control);
        if down && !held {
            self.0.push(control);
        } else if !down {
            self.0.retain(|c| *c != control);
        }
        down && !held
    }

    fn is_held(&self, control: Control) -> bool {
        self.0.contains(&control)
    }
}

#[derive(Default)]
pub struct InputController {
    held: HeldControls,
    mode_chord: bool,
    hotbar_steps: i32,
}

impl InputController {
    pub fn set_control(&mut self, control: Control, down: bool) -> Option<ControlEvent> {
        let pressed = self.held.set(control, down);
        let event = match control {
            Control::Attack => Some(ControlEvent::Attack { down }),
            Control::Interact => Some(ControlEvent::Interact { down }),
            Control::CloseScreen => down.then_some(ControlEvent::CloseScreen),
            Control::SelectHotbar(slot) => down.then_some(ControlEvent::SelectHotbar(slot)),
            _ if !pressed => None,
            Control::HotbarNext => {
                self.hotbar_steps += 1;
                None
            }
            Control::HotbarPrev => {
                self.hotbar_steps -= 1;
                None
            }
            Control::Jump => Some(ControlEvent::JumpPressed),
            Control::ToggleCreative => Some(ControlEvent::ToggleCreative),
            Control::UndoEdit => Some(ControlEvent::UndoEdit),
            Control::RedoEdit => Some(ControlEvent::RedoEdit),
            Control::AdjustToolNext => Some(ControlEvent::AdjustTool(1)),
            Control::AdjustToolPrev => Some(ControlEvent::AdjustTool(-1)),
            Control::ToggleInventory => Some(ControlEvent::ToggleInventory),
            Control::OpenChat => Some(ControlEvent::OpenChat { command: false }),
            Control::OpenCommandChat => Some(ControlEvent::OpenChat { command: true }),
            Control::DropItem => Some(ControlEvent::DropItem),
            Control::SwapOffHand => Some(ControlEvent::SwapOffHand),
            Control::RotateHeldBlock => Some(ControlEvent::RotateHeldBlock),
            Control::TogglePerspective => Some(ControlEvent::TogglePerspective),
            Control::MoveForward
            | Control::MoveBackward
            | Control::MoveLeft
            | Control::MoveRight
            | Control::Sneak
            | Control::Sprint
            | Control::TogglePlayerMode => None,
        };

        event.or_else(|| self.mode_chord_event())
    }

    pub fn sprint_held(&self) -> bool {
        self.held.is_held(Control::Sprint)
    }

    pub fn step_hotbar(&mut self, steps: i32) {
        self.hotbar_steps += steps;
    }

    pub fn take_hotbar_steps(&mut self) -> i32 {
        std::mem::take(&mut self.hotbar_steps)
    }

    pub fn movement(&self) -> MovementInput {
        let held = |control| self.held.is_held(control);
        MovementInput {
            forward: held(Control::MoveForward),
            backward: held(Control::MoveBackward),
            left: held(Control::MoveLeft),
            right: held(Control::MoveRight),
            jump: held(Control::Jump),
            sneak: held(Control::Sneak),
            sprint: held(Control::Sprint),
        }
    }

    fn mode_chord_event(&mut self) -> Option<ControlEvent> {
        let chord = self.sprint_held() && self.held.is_held(Control::TogglePlayerMode);
        let event = chord && !self.mode_chord;
        self.mode_chord = chord;
        event.then_some(ControlEvent::TogglePlayerMode)
    }
}

#[cfg(test)]
mod tests;
