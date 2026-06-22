//! Shared input vocabulary and key bindings.
//!
//! Platform code translates raw device events into these controls, then hands
//! them to `App`. This keeps bindings out of native/web shells and gives future
//! app screens one stable input language.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Control {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    Jump,
    Sneak,
    Sprint,
    ToggleInventory,
    TogglePlayerMode,
    CloseScreen,
    SelectHotbar(u8),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// Map browser-style physical key codes (`KeyboardEvent.code`) to game controls.
/// Native uses the winit-specific helper below so the binding table still lives
/// in this module rather than in `bin/native.rs`.
pub fn control_from_code(code: &str) -> Option<Control> {
    match code {
        "KeyW" => Some(Control::MoveForward),
        "KeyS" => Some(Control::MoveBackward),
        "KeyA" => Some(Control::MoveLeft),
        "KeyD" => Some(Control::MoveRight),
        "Space" => Some(Control::Jump),
        "ShiftLeft" | "ShiftRight" => Some(Control::Sneak),
        "ControlLeft" | "ControlRight" => Some(Control::Sprint),
        "KeyE" => Some(Control::ToggleInventory),
        "KeyY" => Some(Control::TogglePlayerMode),
        "Escape" => Some(Control::CloseScreen),
        "Digit1" => Some(Control::SelectHotbar(0)),
        "Digit2" => Some(Control::SelectHotbar(1)),
        "Digit3" => Some(Control::SelectHotbar(2)),
        "Digit4" => Some(Control::SelectHotbar(3)),
        "Digit5" => Some(Control::SelectHotbar(4)),
        "Digit6" => Some(Control::SelectHotbar(5)),
        "Digit7" => Some(Control::SelectHotbar(6)),
        "Digit8" => Some(Control::SelectHotbar(7)),
        "Digit9" => Some(Control::SelectHotbar(8)),
        _ => None,
    }
}

pub fn control_from_key_code(code: winit::keyboard::KeyCode) -> Option<Control> {
    use winit::keyboard::KeyCode;

    match code {
        KeyCode::KeyW => Some(Control::MoveForward),
        KeyCode::KeyS => Some(Control::MoveBackward),
        KeyCode::KeyA => Some(Control::MoveLeft),
        KeyCode::KeyD => Some(Control::MoveRight),
        KeyCode::Space => Some(Control::Jump),
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(Control::Sneak),
        KeyCode::ControlLeft | KeyCode::ControlRight => Some(Control::Sprint),
        KeyCode::KeyE => Some(Control::ToggleInventory),
        KeyCode::KeyY => Some(Control::TogglePlayerMode),
        KeyCode::Escape => Some(Control::CloseScreen),
        KeyCode::Digit1 => Some(Control::SelectHotbar(0)),
        KeyCode::Digit2 => Some(Control::SelectHotbar(1)),
        KeyCode::Digit3 => Some(Control::SelectHotbar(2)),
        KeyCode::Digit4 => Some(Control::SelectHotbar(3)),
        KeyCode::Digit5 => Some(Control::SelectHotbar(4)),
        KeyCode::Digit6 => Some(Control::SelectHotbar(5)),
        KeyCode::Digit7 => Some(Control::SelectHotbar(6)),
        KeyCode::Digit8 => Some(Control::SelectHotbar(7)),
        KeyCode::Digit9 => Some(Control::SelectHotbar(8)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_codes_map_to_controls() {
        assert_eq!(control_from_code("KeyW"), Some(Control::MoveForward));
        assert_eq!(control_from_code("KeyE"), Some(Control::ToggleInventory));
        assert_eq!(control_from_code("Digit9"), Some(Control::SelectHotbar(8)));
        assert_eq!(control_from_code("Escape"), Some(Control::CloseScreen));
        assert_eq!(control_from_code("KeyQ"), None);
    }
}
