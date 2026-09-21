use crate::game::MovementInput;
use petramond_world::controls::Control;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    ToggleInventory,
    OpenChat {
        command: bool,
    },
    TogglePlayerMode,
    ToggleCreative,
    UndoEdit,
    RedoEdit,
    /// Step whatever is adjustable by this many notches (positive = next).
    AdjustTool(i32),
    JumpPressed,
    CloseScreen,
    SelectHotbar(u8),
    /// Attack / mine state change (both edges — the App mirrors it into the
    /// pointer's break-held/attack-click state, wherever it was bound).
    Attack {
        down: bool,
    },
    /// Interact / place state change (both edges), like [`Attack`](Self::Attack).
    Interact {
        down: bool,
    },
    /// Drop the held item this press (edge-triggered). Whether it's one item
    /// or the whole stack is decided by the App: holding the SPRINT control
    /// (wherever it's bound) drops the stack.
    DropItem,
    /// Swap the selected hotbar stack with the off-hand slot (edge-triggered).
    SwapOffHand,
    RotateHeldBlock,
    TogglePerspective,
}

/// The controls currently down. `Control` is a small closed catalog, so a
/// scan beats hashing.
#[derive(Default)]
struct HeldControls(Vec<Control>);

impl HeldControls {
    /// Record `control`'s state; `true` on the edge from up to down.
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
    /// The sprint + toggle-mode chord is down (its own edge).
    mode_chord: bool,
    /// Whole hotbar steps accumulated by HotbarNext/HotbarPrev edges since the
    /// last frame (positive = next), drained into `GameInput.hotbar_scroll`.
    hotbar_steps: i32,
}

impl InputController {
    pub fn set_control(&mut self, control: Control, down: bool) -> Option<ControlEvent> {
        let pressed = self.held.set(control, down);
        let event = match control {
            // Both edges, wherever they are bound.
            Control::Attack => Some(ControlEvent::Attack { down }),
            Control::Interact => Some(ControlEvent::Interact { down }),
            // Level-triggered: every down report fires, key repeat included.
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
            // Whole stack vs one item is the App's call, read from the held
            // SPRINT state.
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

    /// Whether the SPRINT control is held — the drop-whole-stack modifier
    /// (follows the sprint binding, wherever it points).
    pub fn sprint_held(&self) -> bool {
        self.held.is_held(Control::Sprint)
    }

    /// Step the hotbar as its own controls would (positive = next slot).
    pub fn step_hotbar(&mut self, steps: i32) {
        self.hotbar_steps += steps;
    }

    /// Drain the accumulated hotbar steps (positive = next slot).
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
