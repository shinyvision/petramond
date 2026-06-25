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
    /// Drop the held (active hotbar) item: one item, or the whole stack when the
    /// sprint/Ctrl modifier is held.
    DropItem,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// Keyboard modifier state (Ctrl / Shift), tracked from the OS independently of
/// the game-action keybinds. UI shortcuts key off these physical modifiers — Ctrl
/// for "drop the whole stack", Shift for inventory quick-move — so they stay
/// correct no matter which keys `Sprint` / `Sneak` are bound to. The platform
/// shell updates these from the windowing system's modifier events, not from the
/// rebindable [`Control`] mapping.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
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
        KeyCode::KeyQ => Some(Control::DropItem),
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
