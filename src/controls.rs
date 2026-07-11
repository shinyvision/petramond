//! Shared input vocabulary and key bindings.
//!
//! The app owns a [`BindingSet`] (persisted in `client.json`) mapping each
//! [`BindableAction`] to one [`Binding`] — a key, mouse button, or scroll
//! direction, optionally chorded with held modifiers. Platform code forwards
//! RAW device events to `App`, which resolves them here into [`Control`]s.
//! Controls that are not remappable (Escape, hotbar digits, dev toggles) come
//! from the fixed fallback table [`fixed_control_from_key_code`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Control {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    Jump,
    Sneak,
    Sprint,
    /// Attack / mine (held = keep mining). Default: left mouse button.
    Attack,
    /// Interact / place (held = keep using). Default: right mouse button.
    Interact,
    /// Advance the active hotbar slot by one. Default: scroll down.
    HotbarNext,
    /// Move the active hotbar slot back by one. Default: scroll up.
    HotbarPrev,
    ToggleInventory,
    OpenChat,
    OpenCommandChat,
    TogglePlayerMode,
    CloseScreen,
    SelectHotbar(u8),
    /// Drop the held (active hotbar) item: one item, or the whole stack when the
    /// sprint/Ctrl modifier is held.
    DropItem,
    /// Toggle the held block's placement/render state when it supports rotation.
    RotateHeldBlock,
    /// Toggle between the first-person and third-person camera.
    TogglePerspective,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextKey {
    Backspace,
    Delete,
    Enter,
    Tab,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Home,
    End,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextShortcut {
    SelectAll,
    Cut,
    Copy,
    Paste,
}

/// Keyboard modifier state (Ctrl / Shift / Alt / Meta), tracked from the OS
/// independently of the game-action keybinds. UI shortcuts key off these
/// physical modifiers — Ctrl for "drop the whole stack", Shift for inventory
/// quick-move — so they stay correct no matter which keys `Sprint` / `Sneak`
/// are bound to; binding CHORDS (`Ctrl+B`) match against them too. The platform
/// shell updates these from the windowing system's modifier events, not from the
/// rebindable [`Control`] mapping.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// The controls the player may remap in Options → Controls. Serde names double
/// as widget-id suffixes on the controls screen and as `client.json` keys.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindableAction {
    WalkForward,
    StrafeRight,
    StrafeLeft,
    WalkBackward,
    Jump,
    Attack,
    Interact,
    HotbarNext,
    HotbarPrev,
    OpenInventory,
    Chat,
    Sprint,
    Sneak,
    RotateBlock,
}

impl BindableAction {
    /// Display order of the Options → Controls screen.
    pub const ALL: [BindableAction; 14] = [
        BindableAction::WalkForward,
        BindableAction::StrafeRight,
        BindableAction::StrafeLeft,
        BindableAction::WalkBackward,
        BindableAction::Jump,
        BindableAction::Attack,
        BindableAction::Interact,
        BindableAction::HotbarNext,
        BindableAction::HotbarPrev,
        BindableAction::OpenInventory,
        BindableAction::Chat,
        BindableAction::Sprint,
        BindableAction::Sneak,
        BindableAction::RotateBlock,
    ];

    /// Stable id string (the serde name): widget-id suffix + settings key.
    pub fn id(self) -> &'static str {
        match self {
            BindableAction::WalkForward => "walk_forward",
            BindableAction::StrafeRight => "strafe_right",
            BindableAction::StrafeLeft => "strafe_left",
            BindableAction::WalkBackward => "walk_backward",
            BindableAction::Jump => "jump",
            BindableAction::Attack => "attack",
            BindableAction::Interact => "interact",
            BindableAction::HotbarNext => "hotbar_next",
            BindableAction::HotbarPrev => "hotbar_prev",
            BindableAction::OpenInventory => "open_inventory",
            BindableAction::Chat => "chat",
            BindableAction::Sprint => "sprint",
            BindableAction::Sneak => "sneak",
            BindableAction::RotateBlock => "rotate_block",
        }
    }

    pub fn from_id(id: &str) -> Option<BindableAction> {
        BindableAction::ALL.iter().copied().find(|a| a.id() == id)
    }

    /// The control this action drives when its binding fires.
    pub fn control(self) -> Control {
        match self {
            BindableAction::WalkForward => Control::MoveForward,
            BindableAction::StrafeRight => Control::MoveRight,
            BindableAction::StrafeLeft => Control::MoveLeft,
            BindableAction::WalkBackward => Control::MoveBackward,
            BindableAction::Jump => Control::Jump,
            BindableAction::Attack => Control::Attack,
            BindableAction::Interact => Control::Interact,
            BindableAction::HotbarNext => Control::HotbarNext,
            BindableAction::HotbarPrev => Control::HotbarPrev,
            BindableAction::OpenInventory => Control::ToggleInventory,
            BindableAction::Chat => Control::OpenChat,
            BindableAction::Sprint => Control::Sprint,
            BindableAction::Sneak => Control::Sneak,
            BindableAction::RotateBlock => Control::RotateHeldBlock,
        }
    }

    fn default_binding(self) -> Binding {
        let key = |code| Binding::key(code);
        match self {
            BindableAction::WalkForward => key(KeyCode::KeyW),
            BindableAction::StrafeRight => key(KeyCode::KeyD),
            BindableAction::StrafeLeft => key(KeyCode::KeyA),
            BindableAction::WalkBackward => key(KeyCode::KeyS),
            BindableAction::Jump => key(KeyCode::Space),
            BindableAction::Attack => Binding::mouse(MouseButton::Left),
            BindableAction::Interact => Binding::mouse(MouseButton::Right),
            BindableAction::HotbarNext => Binding::scroll(ScrollDir::Down),
            BindableAction::HotbarPrev => Binding::scroll(ScrollDir::Up),
            BindableAction::OpenInventory => key(KeyCode::KeyE),
            BindableAction::Chat => key(KeyCode::KeyT),
            BindableAction::Sprint => key(KeyCode::ControlLeft),
            BindableAction::Sneak => key(KeyCode::ShiftLeft),
            BindableAction::RotateBlock => key(KeyCode::KeyR),
        }
    }
}

/// One wheel-notch direction, as a bindable input.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDir {
    Up,
    Down,
}

/// The device input a binding listens for. Serialized externally tagged, so a
/// `client.json` binding reads as `{"key": "KeyW"}`, `{"mouse": "Left"}`, or
/// `{"scroll": "down"}`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundInput {
    Key(KeyCode),
    Mouse(MouseButton),
    Scroll(ScrollDir),
}

/// Modifiers a chord binding requires to be HELD when its input fires.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BindMods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl BindMods {
    pub fn is_empty(&self) -> bool {
        *self == BindMods::default()
    }

    pub fn from_modifiers(m: Modifiers) -> BindMods {
        BindMods {
            ctrl: m.ctrl,
            shift: m.shift,
            alt: m.alt,
            meta: m.meta,
        }
    }

    /// Whether every required modifier is currently held.
    fn satisfied_by(&self, m: Modifiers) -> bool {
        (!self.ctrl || m.ctrl)
            && (!self.shift || m.shift)
            && (!self.alt || m.alt)
            && (!self.meta || m.meta)
    }

    fn count(&self) -> u32 {
        self.ctrl as u32 + self.shift as u32 + self.alt as u32 + self.meta as u32
    }
}

/// One remappable binding: a device input plus the modifier chord (empty for
/// a plain key/button/notch).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    #[serde(default, skip_serializing_if = "BindMods::is_empty")]
    pub mods: BindMods,
    #[serde(flatten)]
    pub input: BoundInput,
}

impl Binding {
    pub fn key(code: KeyCode) -> Binding {
        Binding {
            mods: BindMods::default(),
            input: BoundInput::Key(code),
        }
    }

    pub fn mouse(button: MouseButton) -> Binding {
        Binding {
            mods: BindMods::default(),
            input: BoundInput::Mouse(button),
        }
    }

    pub fn scroll(dir: ScrollDir) -> Binding {
        Binding {
            mods: BindMods::default(),
            input: BoundInput::Scroll(dir),
        }
    }

    /// Short display text for the controls screen ("W", "CTRL + B",
    /// "LEFT CLICK", "SCROLL UP").
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.mods.ctrl {
            parts.push("CTRL");
        }
        if self.mods.shift {
            parts.push("SHIFT");
        }
        if self.mods.alt {
            parts.push("ALT");
        }
        if self.mods.meta {
            parts.push("META");
        }
        let input = match self.input {
            BoundInput::Key(code) => key_label(code),
            BoundInput::Mouse(button) => mouse_label(button),
            BoundInput::Scroll(ScrollDir::Up) => "SCROLL UP".to_string(),
            BoundInput::Scroll(ScrollDir::Down) => "SCROLL DOWN".to_string(),
        };
        if parts.is_empty() {
            input
        } else {
            format!("{} + {input}", parts.join(" + "))
        }
    }
}

fn mouse_label(button: MouseButton) -> String {
    match button {
        MouseButton::Left => "LEFT CLICK".to_string(),
        MouseButton::Right => "RIGHT CLICK".to_string(),
        MouseButton::Middle => "MIDDLE CLICK".to_string(),
        MouseButton::Back => "MOUSE BACK".to_string(),
        MouseButton::Forward => "MOUSE FORWARD".to_string(),
        MouseButton::Other(n) => format!("MOUSE {n}"),
    }
}

fn key_label(code: KeyCode) -> String {
    let name = match code {
        KeyCode::ShiftLeft => "LEFT SHIFT",
        KeyCode::ShiftRight => "RIGHT SHIFT",
        KeyCode::ControlLeft => "LEFT CTRL",
        KeyCode::ControlRight => "RIGHT CTRL",
        KeyCode::AltLeft => "LEFT ALT",
        KeyCode::AltRight => "RIGHT ALT",
        KeyCode::SuperLeft => "LEFT META",
        KeyCode::SuperRight => "RIGHT META",
        KeyCode::Space => "SPACE",
        KeyCode::Enter => "ENTER",
        KeyCode::Tab => "TAB",
        KeyCode::Backspace => "BACKSPACE",
        KeyCode::ArrowUp => "UP",
        KeyCode::ArrowDown => "DOWN",
        KeyCode::ArrowLeft => "LEFT",
        KeyCode::ArrowRight => "RIGHT",
        _ => {
            // "KeyW" → "W", "Digit3" → "3", anything else keeps its
            // (uppercased) winit name.
            let debug = format!("{code:?}");
            let stripped = debug
                .strip_prefix("Key")
                .or_else(|| debug.strip_prefix("Digit"))
                .unwrap_or(&debug);
            return stripped.to_uppercase();
        }
    };
    name.to_string()
}

/// Whether `code` is a modifier key — remap capture treats these specially
/// (chord starters that bind bare on tap-release).
pub fn is_modifier_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

/// The per-player action → binding table. Missing actions fall back to their
/// defaults, so hand-edited or older `client.json` files stay valid.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BindingSet {
    map: BTreeMap<BindableAction, Binding>,
}

impl BindingSet {
    pub fn binding(&self, action: BindableAction) -> Binding {
        self.map
            .get(&action)
            .copied()
            .unwrap_or_else(|| action.default_binding())
    }

    pub fn set(&mut self, action: BindableAction, binding: Binding) {
        self.map.insert(action, binding);
    }

    /// The actions `input` fires under the currently held `mods`: a binding
    /// matches when its input matches and its required modifiers are all held.
    /// When bindings on the same input differ in specificity (`B` vs `Ctrl+B`),
    /// only the most specific satisfied chord(s) fire.
    pub fn matches(&self, input: BoundInput, mods: Modifiers) -> Vec<BindableAction> {
        let satisfied: Vec<(BindableAction, u32)> = BindableAction::ALL
            .iter()
            .copied()
            .filter_map(|action| {
                let b = self.binding(action);
                (b.input == input && b.mods.satisfied_by(mods)).then_some((action, b.mods.count()))
            })
            .collect();
        let best = satisfied.iter().map(|(_, n)| *n).max().unwrap_or(0);
        satisfied
            .into_iter()
            .filter_map(|(action, n)| (n == best).then_some(action))
            .collect()
    }
}

/// Tracks which bound actions are currently DOWN, so releases resolve by the
/// input that pressed them (a chord's modifier may lift before its key) and a
/// held control never sticks. App-owned, never tick-visible.
#[derive(Default)]
pub struct BindingEngine {
    active: Vec<(BindableAction, BoundInput, BindMods)>,
}

impl BindingEngine {
    /// Resolve one raw input edge into `(Control, down)` transitions.
    pub fn on_input(
        &mut self,
        set: &BindingSet,
        input: BoundInput,
        down: bool,
        mods: Modifiers,
        out: &mut Vec<(Control, bool)>,
    ) {
        if down {
            for action in set.matches(input, mods) {
                if self.active.iter().any(|(a, ..)| *a == action) {
                    continue; // key repeat
                }
                self.active.push((action, input, set.binding(action).mods));
                out.push((action.control(), true));
            }
        } else {
            self.active.retain(|(action, held_input, _)| {
                let release = *held_input == input;
                if release {
                    out.push((action.control(), false));
                }
                !release
            });
        }
    }

    /// A modifier lifted: release every active chord whose required modifiers
    /// are no longer held (`Ctrl+B` sprint must stop when Ctrl lifts, even
    /// while B stays down).
    pub fn on_modifiers_changed(&mut self, mods: Modifiers, out: &mut Vec<(Control, bool)>) {
        self.active.retain(|(action, _, required)| {
            let release = !required.satisfied_by(mods);
            if release {
                out.push((action.control(), false));
            }
            !release
        });
    }

    /// Release everything (focus loss, screen teardown).
    pub fn release_all(&mut self, out: &mut Vec<(Control, bool)>) {
        for (action, ..) in self.active.drain(..) {
            out.push((action.control(), false));
        }
    }
}

/// The FIXED (non-remappable) key table: everything the Options screen does
/// not expose. Consulted only when no player binding matched the key.
pub fn fixed_control_from_key_code(code: KeyCode) -> Option<Control> {
    match code {
        KeyCode::Slash => Some(Control::OpenCommandChat),
        KeyCode::KeyY => Some(Control::TogglePlayerMode),
        KeyCode::KeyQ => Some(Control::DropItem),
        // Plain V only — Ctrl+V stays the text-input paste shortcut.
        KeyCode::KeyV => Some(Control::TogglePerspective),
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
mod binding_tests {
    use super::*;

    fn mods(ctrl: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            ..Modifiers::default()
        }
    }

    #[test]
    fn defaults_cover_every_action_and_roundtrip_serde() {
        let set = BindingSet::default();
        for action in BindableAction::ALL {
            let _ = set.binding(action); // no panic, always a binding
        }
        // A customized set survives a JSON round-trip (the client.json path).
        let mut set = set;
        set.set(
            BindableAction::Sprint,
            Binding {
                mods: BindMods {
                    ctrl: true,
                    ..BindMods::default()
                },
                input: BoundInput::Key(KeyCode::KeyB),
            },
        );
        set.set(BindableAction::Attack, Binding::scroll(ScrollDir::Up));
        let json = serde_json::to_string(&set).expect("serialize");
        let back: BindingSet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, set);
        // Actions absent from the map still resolve to their defaults.
        assert_eq!(
            back.binding(BindableAction::Jump),
            Binding::key(KeyCode::Space)
        );
    }

    #[test]
    fn chord_match_prefers_the_most_specific_binding() {
        let mut set = BindingSet::default();
        set.set(BindableAction::Jump, Binding::key(KeyCode::KeyB));
        set.set(
            BindableAction::Sprint,
            Binding {
                mods: BindMods {
                    ctrl: true,
                    ..BindMods::default()
                },
                input: BoundInput::Key(KeyCode::KeyB),
            },
        );
        // Plain B: only the unchorded binding.
        assert_eq!(
            set.matches(BoundInput::Key(KeyCode::KeyB), mods(false, false)),
            vec![BindableAction::Jump]
        );
        // Ctrl+B: the chord wins over the plain binding.
        assert_eq!(
            set.matches(BoundInput::Key(KeyCode::KeyB), mods(true, false)),
            vec![BindableAction::Sprint]
        );
        // A required-but-unheld modifier never fires.
        assert_eq!(
            set.matches(BoundInput::Key(KeyCode::KeyB), mods(false, true)),
            vec![BindableAction::Jump]
        );
    }

    #[test]
    fn engine_releases_by_input_and_on_modifier_lift() {
        let mut set = BindingSet::default();
        set.set(
            BindableAction::Sprint,
            Binding {
                mods: BindMods {
                    ctrl: true,
                    ..BindMods::default()
                },
                input: BoundInput::Key(KeyCode::KeyB),
            },
        );
        let mut engine = BindingEngine::default();
        let mut out = Vec::new();

        engine.on_input(
            &set,
            BoundInput::Key(KeyCode::KeyB),
            true,
            mods(true, false),
            &mut out,
        );
        assert_eq!(out, vec![(Control::Sprint, true)]);
        out.clear();

        // Repeat (key auto-repeat) does not re-fire.
        engine.on_input(
            &set,
            BoundInput::Key(KeyCode::KeyB),
            true,
            mods(true, false),
            &mut out,
        );
        assert!(out.is_empty());

        // Ctrl lifting releases the chord even while B stays down.
        engine.on_modifiers_changed(mods(false, false), &mut out);
        assert_eq!(out, vec![(Control::Sprint, false)]);
        out.clear();

        // The later B release no longer refers to an active action.
        engine.on_input(
            &set,
            BoundInput::Key(KeyCode::KeyB),
            false,
            mods(false, false),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn engine_releases_a_plain_key_even_if_modifiers_changed_mid_hold() {
        let set = BindingSet::default();
        let mut engine = BindingEngine::default();
        let mut out = Vec::new();
        engine.on_input(
            &set,
            BoundInput::Key(KeyCode::KeyW),
            true,
            mods(false, false),
            &mut out,
        );
        assert_eq!(out, vec![(Control::MoveForward, true)]);
        out.clear();
        // Holding Ctrl (sprint) must not release W; unchorded bindings ignore
        // modifier changes.
        engine.on_modifiers_changed(mods(true, false), &mut out);
        assert!(out.is_empty());
        engine.on_input(
            &set,
            BoundInput::Key(KeyCode::KeyW),
            false,
            mods(true, false),
            &mut out,
        );
        assert_eq!(out, vec![(Control::MoveForward, false)]);
    }

    #[test]
    fn binding_labels_read_naturally() {
        assert_eq!(Binding::key(KeyCode::KeyW).label(), "W");
        assert_eq!(Binding::key(KeyCode::Digit3).label(), "3");
        assert_eq!(Binding::key(KeyCode::ControlLeft).label(), "LEFT CTRL");
        assert_eq!(Binding::mouse(MouseButton::Left).label(), "LEFT CLICK");
        assert_eq!(Binding::scroll(ScrollDir::Up).label(), "SCROLL UP");
        assert_eq!(
            Binding {
                mods: BindMods {
                    ctrl: true,
                    shift: true,
                    ..BindMods::default()
                },
                input: BoundInput::Key(KeyCode::KeyB),
            }
            .label(),
            "CTRL + SHIFT + B"
        );
    }
}

pub fn text_key_from_named(key: &winit::keyboard::NamedKey) -> Option<TextKey> {
    use winit::keyboard::NamedKey;

    match key {
        NamedKey::Backspace => Some(TextKey::Backspace),
        NamedKey::Delete => Some(TextKey::Delete),
        NamedKey::Enter => Some(TextKey::Enter),
        NamedKey::Tab => Some(TextKey::Tab),
        NamedKey::ArrowLeft => Some(TextKey::ArrowLeft),
        NamedKey::ArrowRight => Some(TextKey::ArrowRight),
        NamedKey::ArrowUp => Some(TextKey::ArrowUp),
        NamedKey::ArrowDown => Some(TextKey::ArrowDown),
        NamedKey::Home => Some(TextKey::Home),
        NamedKey::End => Some(TextKey::End),
        _ => None,
    }
}

pub fn text_shortcut_from_key_code(
    code: winit::keyboard::KeyCode,
    modifiers: Modifiers,
) -> Option<TextShortcut> {
    use winit::keyboard::KeyCode;

    if !modifiers.ctrl {
        return None;
    }

    match code {
        KeyCode::KeyA => Some(TextShortcut::SelectAll),
        KeyCode::KeyX => Some(TextShortcut::Cut),
        KeyCode::KeyC => Some(TextShortcut::Copy),
        KeyCode::KeyV => Some(TextShortcut::Paste),
        _ => None,
    }
}
