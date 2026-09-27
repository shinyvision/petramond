use std::collections::BTreeMap;

use crate::keycode::{KeyCode, MouseButton};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Control {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    Jump,
    Sneak,
    Sprint,
    Attack,
    Interact,
    HotbarNext,
    HotbarPrev,
    ToggleInventory,
    OpenChat,
    OpenCommandChat,
    TogglePlayerMode,
    ToggleCreative,
    UndoEdit,
    RedoEdit,
    AdjustToolNext,
    AdjustToolPrev,
    CloseScreen,
    SelectHotbar(u8),
    DropItem,
    SwapOffHand,
    RotateHeldBlock,
    TogglePerspective,
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
    F(u8),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextShortcut {
    SelectAll,
    Cut,
    Copy,
    Paste,
    Chord(char),
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindableAction {
    WalkForward,
    StrafeRight,
    StrafeLeft,
    WalkBackward,
    Jump,
    Sprint,
    Sneak,
    Attack,
    Interact,
    OpenInventory,
    HotbarNext,
    HotbarPrev,
    RotateBlock,
    DropItem,
    SwapOffHand,
    Chat,
    CreativeMode,
    UndoEdit,
    RedoEdit,
    AdjustToolNext,
    AdjustToolPrev,
}

impl BindableAction {
    pub const ALL: [BindableAction; 21] = [
        BindableAction::WalkForward,
        BindableAction::StrafeRight,
        BindableAction::StrafeLeft,
        BindableAction::WalkBackward,
        BindableAction::Jump,
        BindableAction::Sprint,
        BindableAction::Sneak,
        BindableAction::Attack,
        BindableAction::Interact,
        BindableAction::OpenInventory,
        BindableAction::HotbarNext,
        BindableAction::HotbarPrev,
        BindableAction::RotateBlock,
        BindableAction::DropItem,
        BindableAction::SwapOffHand,
        BindableAction::Chat,
        BindableAction::CreativeMode,
        BindableAction::UndoEdit,
        BindableAction::RedoEdit,
        BindableAction::AdjustToolNext,
        BindableAction::AdjustToolPrev,
    ];

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
            BindableAction::CreativeMode => "creative_mode",
            BindableAction::UndoEdit => "undo_edit",
            BindableAction::RedoEdit => "redo_edit",
            BindableAction::AdjustToolNext => "adjust_tool_next",
            BindableAction::AdjustToolPrev => "adjust_tool_prev",
            BindableAction::Sprint => "sprint",
            BindableAction::Sneak => "sneak",
            BindableAction::RotateBlock => "rotate_block",
            BindableAction::DropItem => "drop_item",
            BindableAction::SwapOffHand => "swap_off_hand",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BindableAction::WalkForward => "Walk Forward",
            BindableAction::StrafeRight => "Strafe Right",
            BindableAction::StrafeLeft => "Strafe Left",
            BindableAction::WalkBackward => "Walk Backward",
            BindableAction::Jump => "Jump",
            BindableAction::Attack => "Attack / Mine",
            BindableAction::Interact => "Interact",
            BindableAction::HotbarNext => "Next Hotbar",
            BindableAction::HotbarPrev => "Previous Hotbar",
            BindableAction::OpenInventory => "Open Inventory",
            BindableAction::Chat => "Chat",
            BindableAction::CreativeMode => "Creative Mode",
            BindableAction::UndoEdit => "Undo Edit",
            BindableAction::RedoEdit => "Redo Edit",
            BindableAction::AdjustToolNext => "Next Tool Setting",
            BindableAction::AdjustToolPrev => "Prev Tool Setting",
            BindableAction::Sprint => "Sprint",
            BindableAction::Sneak => "Sneak",
            BindableAction::RotateBlock => "Rotate Block",
            BindableAction::DropItem => "Drop Item",
            BindableAction::SwapOffHand => "Swap Off-hand",
        }
    }

    pub fn category(self) -> &'static str {
        match self {
            BindableAction::WalkForward
            | BindableAction::StrafeRight
            | BindableAction::StrafeLeft
            | BindableAction::WalkBackward
            | BindableAction::Jump
            | BindableAction::Sprint
            | BindableAction::Sneak => "Movement",
            BindableAction::Attack
            | BindableAction::Interact
            | BindableAction::OpenInventory
            | BindableAction::HotbarNext
            | BindableAction::HotbarPrev
            | BindableAction::RotateBlock
            | BindableAction::DropItem
            | BindableAction::SwapOffHand => "Interacting",
            BindableAction::Chat
            | BindableAction::CreativeMode
            | BindableAction::UndoEdit
            | BindableAction::RedoEdit
            | BindableAction::AdjustToolNext
            | BindableAction::AdjustToolPrev => "Other",
        }
    }

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
            BindableAction::CreativeMode => Control::ToggleCreative,
            BindableAction::UndoEdit => Control::UndoEdit,
            BindableAction::RedoEdit => Control::RedoEdit,
            BindableAction::AdjustToolNext => Control::AdjustToolNext,
            BindableAction::AdjustToolPrev => Control::AdjustToolPrev,
            BindableAction::Sprint => Control::Sprint,
            BindableAction::Sneak => Control::Sneak,
            BindableAction::RotateBlock => Control::RotateHeldBlock,
            BindableAction::DropItem => Control::DropItem,
            BindableAction::SwapOffHand => Control::SwapOffHand,
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
            BindableAction::CreativeMode | BindableAction::UndoEdit | BindableAction::RedoEdit => {
                Binding {
                    mods: BindMods {
                        ctrl: true,
                        shift: self == BindableAction::RedoEdit,
                        ..BindMods::default()
                    },
                    ..key(if self == BindableAction::CreativeMode {
                        KeyCode::KeyU
                    } else {
                        KeyCode::KeyZ
                    })
                }
            }
            BindableAction::AdjustToolNext | BindableAction::AdjustToolPrev => Binding {
                mods: BindMods {
                    ctrl: true,
                    ..BindMods::default()
                },
                ..Binding::scroll(if self == BindableAction::AdjustToolNext {
                    ScrollDir::Down
                } else {
                    ScrollDir::Up
                })
            },
            BindableAction::Sprint => key(KeyCode::ControlLeft),
            BindableAction::Sneak => key(KeyCode::ShiftLeft),
            BindableAction::RotateBlock => key(KeyCode::KeyR),
            BindableAction::DropItem => key(KeyCode::KeyQ),
            BindableAction::SwapOffHand => key(KeyCode::KeyF),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDir {
    Up,
    Down,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundInput {
    Key(KeyCode),
    Mouse(MouseButton),
    Scroll(ScrollDir),
}

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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BindingSet {
    map: BTreeMap<String, Binding>,
}

impl BindingSet {
    pub fn get(&self, id: &str) -> Option<Binding> {
        self.map.get(id).copied()
    }

    pub fn set_id(&mut self, id: &str, binding: Binding) {
        self.map.insert(id.to_string(), binding);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set(&mut self, action: BindableAction, binding: Binding) {
        self.set_id(action.id(), binding);
    }

    pub fn binding(&self, action: BindableAction) -> Binding {
        self.get(action.id())
            .unwrap_or_else(|| action.default_binding())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActionOut {
    Control(Control),
    ClientMod(String),
}

pub struct ActionRow {
    pub id: String,
    pub label: String,
    pub category: String,
    pub default: Binding,
    target: ActionTarget,
}

enum ActionTarget {
    Control(Control),
    ClientMod,
}

impl ActionRow {
    pub fn is_mod(&self) -> bool {
        matches!(self.target, ActionTarget::ClientMod)
    }
}

pub struct ActionTable {
    rows: Vec<ActionRow>,
}

impl ActionTable {
    pub fn engine() -> ActionTable {
        ActionTable {
            rows: BindableAction::ALL
                .iter()
                .map(|a| ActionRow {
                    id: a.id().to_string(),
                    label: a.label().to_string(),
                    category: a.category().to_string(),
                    default: a.default_binding(),
                    target: ActionTarget::Control(a.control()),
                })
                .collect(),
        }
    }

    pub fn push_registered_action(
        &mut self,
        id: String,
        label: String,
        category: String,
        default: Binding,
    ) {
        self.rows.push(ActionRow {
            id,
            label,
            category,
            default,
            target: ActionTarget::ClientMod,
        });
    }

    pub fn rows(&self) -> &[ActionRow] {
        &self.rows
    }

    pub fn row(&self, id: &str) -> Option<&ActionRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn effective(&self, set: &BindingSet, row: &ActionRow) -> Binding {
        set.get(&row.id).unwrap_or(row.default)
    }

    fn matches(
        &self,
        set: &BindingSet,
        input: BoundInput,
        mods: Modifiers,
        live: &dyn Fn(&ActionRow) -> bool,
    ) -> Vec<usize> {
        let satisfied: Vec<(usize, u32)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                let b = self.effective(set, row);
                (b.input == input && b.mods.satisfied_by(mods) && live(row))
                    .then_some((i, b.mods.count()))
            })
            .collect();
        let best = satisfied.iter().map(|(_, n)| *n).max().unwrap_or(0);
        satisfied
            .into_iter()
            .filter_map(|(i, n)| (n == best).then_some(i))
            .collect()
    }

    fn out_for(&self, row: &ActionRow) -> ActionOut {
        match row.target {
            ActionTarget::Control(control) => ActionOut::Control(control),
            ActionTarget::ClientMod => ActionOut::ClientMod(row.id.clone()),
        }
    }
}

struct ActiveBind {
    id: String,
    out: ActionOut,
    input: BoundInput,
    required: BindMods,
}

#[derive(Default)]
pub struct BindingEngine {
    active: Vec<ActiveBind>,
}

impl BindingEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn on_input(
        &mut self,
        table: &ActionTable,
        set: &BindingSet,
        input: BoundInput,
        down: bool,
        mods: Modifiers,
        live: &dyn Fn(&ActionRow) -> bool,
        out: &mut Vec<(ActionOut, bool)>,
    ) {
        if down {
            for i in table.matches(set, input, mods, live) {
                let row = &table.rows()[i];
                if self.active.iter().any(|a| a.id == row.id) {
                    continue;
                }
                let fired = table.out_for(row);
                out.push((fired.clone(), true));
                self.active.push(ActiveBind {
                    id: row.id.clone(),
                    out: fired,
                    input,
                    required: table.effective(set, row).mods,
                });
            }
        } else {
            self.active.retain(|a| {
                let release = a.input == input;
                if release {
                    out.push((a.out.clone(), false));
                }
                !release
            });
        }
    }

    pub fn on_modifiers_changed(&mut self, mods: Modifiers, out: &mut Vec<(ActionOut, bool)>) {
        self.active.retain(|a| {
            let release = !a.required.satisfied_by(mods);
            if release {
                out.push((a.out.clone(), false));
            }
            !release
        });
    }

    pub fn release_all(&mut self, out: &mut Vec<(ActionOut, bool)>) {
        for a in self.active.drain(..) {
            out.push((a.out, false));
        }
    }
}

pub fn fixed_control_from_key_code(code: KeyCode) -> Option<Control> {
    match code {
        KeyCode::Slash => Some(Control::OpenCommandChat),
        KeyCode::KeyY => Some(Control::TogglePlayerMode),
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

pub fn text_shortcut_from_key_code(code: KeyCode, modifiers: Modifiers) -> Option<TextShortcut> {
    if !modifiers.ctrl {
        return None;
    }

    match code {
        KeyCode::KeyA => Some(TextShortcut::SelectAll),
        KeyCode::KeyX => Some(TextShortcut::Cut),
        KeyCode::KeyC => Some(TextShortcut::Copy),
        KeyCode::KeyV => Some(TextShortcut::Paste),
        code => chord_char(code).map(TextShortcut::Chord),
    }
}

fn chord_char(code: KeyCode) -> Option<char> {
    let name = code.name();
    let ch = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .filter(|rest| rest.len() == 1)?
        .chars()
        .next()?;
    Some(ch.to_ascii_lowercase())
}

#[cfg(test)]
mod binding_tests {
    use super::*;

    #[test]
    fn ctrl_with_any_other_letter_or_digit_is_a_chord() {
        let ctrl = mods(true, false);
        assert_eq!(
            text_shortcut_from_key_code(KeyCode::KeyC, ctrl),
            Some(TextShortcut::Copy)
        );
        assert_eq!(
            text_shortcut_from_key_code(KeyCode::KeyS, ctrl),
            Some(TextShortcut::Chord('s'))
        );
        assert_eq!(
            text_shortcut_from_key_code(KeyCode::Digit5, ctrl),
            Some(TextShortcut::Chord('5'))
        );
        assert_eq!(text_shortcut_from_key_code(KeyCode::F1, ctrl), None);
        assert_eq!(
            text_shortcut_from_key_code(KeyCode::KeyS, mods(false, false)),
            None
        );
    }

    fn mods(ctrl: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            shift,
            ..Modifiers::default()
        }
    }

    fn match_ids(
        table: &ActionTable,
        set: &BindingSet,
        input: BoundInput,
        m: Modifiers,
    ) -> Vec<String> {
        table
            .matches(set, input, m, &|_| true)
            .into_iter()
            .map(|i| table.rows()[i].id.clone())
            .collect()
    }

    #[test]
    fn defaults_cover_every_action_and_roundtrip_serde() {
        let set = BindingSet::default();
        for action in BindableAction::ALL {
            let _ = set.binding(action);
        }
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
        set.set_id("minimap:open_map", Binding::key(KeyCode::KeyO));
        let json = serde_json::to_string(&set).expect("serialize");
        let back: BindingSet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, set);
        assert_eq!(
            back.binding(BindableAction::Jump),
            Binding::key(KeyCode::Space)
        );
        assert_eq!(
            back.get("minimap:open_map"),
            Some(Binding::key(KeyCode::KeyO))
        );
    }

    #[test]
    fn a_chord_that_does_not_fire_here_leaves_the_plain_key_alone() {
        let mut table = ActionTable::engine();
        table.push_registered_action(
            "studio:duplicate".into(),
            "Duplicate".into(),
            "Studio".into(),
            Binding {
                mods: BindMods {
                    ctrl: true,
                    ..BindMods::default()
                },
                ..Binding::key(KeyCode::KeyD)
            },
        );
        let set = BindingSet::default();
        let fired = |live: &dyn Fn(&ActionRow) -> bool| -> Vec<String> {
            table
                .matches(
                    &set,
                    BoundInput::Key(KeyCode::KeyD),
                    mods(true, false),
                    live,
                )
                .into_iter()
                .map(|i| table.rows()[i].id.clone())
                .collect()
        };
        assert_eq!(fired(&|row| !row.is_mod()), ["strafe_right"]);
        assert_eq!(
            fired(&|_| true),
            ["studio:duplicate"],
            "where it fires, the chord wins"
        );
    }

    #[test]
    fn chord_match_prefers_the_most_specific_binding() {
        let table = ActionTable::engine();
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
        assert_eq!(
            match_ids(
                &table,
                &set,
                BoundInput::Key(KeyCode::KeyB),
                mods(false, false)
            ),
            vec!["jump"]
        );
        assert_eq!(
            match_ids(
                &table,
                &set,
                BoundInput::Key(KeyCode::KeyB),
                mods(true, false)
            ),
            vec!["sprint"]
        );
        assert_eq!(
            match_ids(
                &table,
                &set,
                BoundInput::Key(KeyCode::KeyB),
                mods(false, true)
            ),
            vec!["jump"]
        );
    }

    #[test]
    fn engine_releases_by_input_and_on_modifier_lift() {
        let table = ActionTable::engine();
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
            &table,
            &set,
            BoundInput::Key(KeyCode::KeyB),
            true,
            mods(true, false),
            &|_| true,
            &mut out,
        );
        assert_eq!(out, vec![(ActionOut::Control(Control::Sprint), true)]);
        out.clear();

        engine.on_input(
            &table,
            &set,
            BoundInput::Key(KeyCode::KeyB),
            true,
            mods(true, false),
            &|_| true,
            &mut out,
        );
        assert!(out.is_empty());

        engine.on_modifiers_changed(mods(false, false), &mut out);
        assert_eq!(out, vec![(ActionOut::Control(Control::Sprint), false)]);
        out.clear();

        engine.on_input(
            &table,
            &set,
            BoundInput::Key(KeyCode::KeyB),
            false,
            mods(false, false),
            &|_| true,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn mod_actions_resolve_and_release_after_a_table_swap() {
        let mut table = ActionTable::engine();
        table.push_registered_action(
            "minimap:open_map".into(),
            "Open World Map".into(),
            "Minimap".into(),
            Binding::key(KeyCode::KeyM),
        );
        let set = BindingSet::default();
        let mut engine = BindingEngine::default();
        let mut out = Vec::new();

        engine.on_input(
            &table,
            &set,
            BoundInput::Key(KeyCode::KeyM),
            true,
            mods(false, false),
            &|_| true,
            &mut out,
        );
        assert_eq!(
            out,
            vec![(ActionOut::ClientMod("minimap:open_map".into()), true)]
        );
        out.clear();

        let engine_only = ActionTable::engine();
        engine.on_input(
            &engine_only,
            &set,
            BoundInput::Key(KeyCode::KeyM),
            false,
            mods(false, false),
            &|_| true,
            &mut out,
        );
        assert_eq!(
            out,
            vec![(ActionOut::ClientMod("minimap:open_map".into()), false)]
        );
    }

    #[test]
    fn engine_releases_a_plain_key_even_if_modifiers_changed_mid_hold() {
        let table = ActionTable::engine();
        let set = BindingSet::default();
        let mut engine = BindingEngine::default();
        let mut out = Vec::new();
        engine.on_input(
            &table,
            &set,
            BoundInput::Key(KeyCode::KeyW),
            true,
            mods(false, false),
            &|_| true,
            &mut out,
        );
        assert_eq!(out, vec![(ActionOut::Control(Control::MoveForward), true)]);
        out.clear();
        engine.on_modifiers_changed(mods(true, false), &mut out);
        assert!(out.is_empty());
        engine.on_input(
            &table,
            &set,
            BoundInput::Key(KeyCode::KeyW),
            false,
            mods(true, false),
            &|_| true,
            &mut out,
        );
        assert_eq!(out, vec![(ActionOut::Control(Control::MoveForward), false)]);
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
