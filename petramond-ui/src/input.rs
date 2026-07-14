//! Host input events, per-GUI ephemeral widget state, and resolved UI events.
//!
//! [`FrameState`] is everything that must persist across frames while a GUI
//! is open but must NEVER cross into game/tick state: hover, press, focus,
//! scroll offsets, text editors, drags. The host owns one per open GUI and
//! drops it on close. The only artifacts that may reach a deterministic tick
//! are the [`UiEvent`]s the host chooses to latch.

use crate::text_edit::TextInput;
use crate::tree::InstKey;
use std::collections::BTreeMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// Semantic (host-keymapped) keys: the host translates ctrl+C → `Copy` etc.,
/// so petramond-ui never owns a keymap.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NavKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    Delete,
    Backspace,
    Home,
    End,
    SelectAll,
    Copy,
    Cut,
    Paste,
}

/// One host input event. Pointer coordinates are physical px.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum InputEvent {
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerDown {
        x: f32,
        y: f32,
        button: PointerButton,
        shift: bool,
        /// The host says a cursor-held item may start a slot-distribution
        /// gesture. The runtime then captures distinct slot cells until the
        /// matching release instead of firing the initial slot immediately.
        slot_drag: bool,
    },
    PointerUp {
        x: f32,
        y: f32,
        button: PointerButton,
    },
    /// Wheel scroll; positive = content moves up/left (natural list scroll),
    /// in logical px.
    Scroll {
        delta: i32,
    },
    Key {
        key: NavKey,
        shift: bool,
    },
    Char {
        ch: char,
    },
    /// Pointer/keyboard focus left the window: release presses and drags.
    Blur,
}

/// A resolved widget event the host acts on. `item` is the list item index
/// when the widget lives inside a list template.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Click {
        id: String,
        item: Option<u32>,
        button: PointerButton,
    },
    Toggle {
        id: String,
        item: Option<u32>,
        on: bool,
    },
    SliderChange {
        id: String,
        item: Option<u32>,
        value: f32,
        /// `false` while dragging (live preview), `true` on release.
        committed: bool,
    },
    TextChanged {
        id: String,
        text: String,
    },
    Submit {
        id: String,
        text: String,
    },
    /// Pointer interaction over an `image` with `interactive: true`.
    /// Coordinates are local to the solved image rect in logical pixels and
    /// remain available while a drag continues outside the rect.
    ImagePointer {
        id: String,
        phase: PointerPhase,
        x: f32,
        y: f32,
        button: PointerButton,
    },
    ListSelect {
        id: String,
        index: u32,
    },
    /// Double-click / Enter on a list row.
    ListActivate {
        id: String,
        index: u32,
    },
    /// An ordinary pointer activation on a slot cell. The host maps
    /// `(role, index)` to its own slot identity and latches it to the tick.
    SlotClick {
        role: String,
        index: u32,
        button: PointerButton,
        shift: bool,
    },
    /// A cursor-held stack dragged across two or more distinct slot cells.
    /// Cells stay in first-hit order and never repeat within one press; the
    /// host owns the item-distribution policy and authoritative mutation.
    SlotDrag {
        slots: Vec<(String, u32)>,
        button: PointerButton,
    },
    /// A press that hit nothing, outside the root panel (cursor-stack throw).
    ClickOutside {
        button: PointerButton,
    },
    /// A key no widget consumed — per-screen controllers handle these
    /// (world-select's Delete jump, ESC-back, list keyboard nav).
    Key {
        key: NavKey,
        shift: bool,
    },
}

/// What kind of drag the pointer currently owns.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Drag {
    Slider {
        key: InstKey,
    },
    ScrollThumb {
        key: InstKey,
        /// Pointer offset within the thumb at grab, physical px.
        grab: f32,
    },
    TextSelect {
        key: InstKey,
        anchor: usize,
    },
    Image {
        key: InstKey,
        button: PointerButton,
    },
    Slots {
        button: PointerButton,
        shift: bool,
        /// Distinct `(role, in-role index)` cells in first-hit order.
        slots: Vec<(String, u32)>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
}

/// Per-open-GUI ephemeral state. Never serialized, never tick-visible.
#[derive(Debug, Default)]
pub struct FrameState {
    /// Host time in seconds (drives blink and double-click windows).
    pub now: f64,
    pub(crate) cursor: (f32, f32),
    /// The pressed widget: set on PointerDown over an event widget, cleared
    /// on PointerUp (press-in-release-in click semantics).
    pub(crate) active: Option<(InstKey, PointerButton)>,
    pub(crate) focus: Option<InstKey>,
    pub(crate) scroll: BTreeMap<InstKey, i32>,
    pub(crate) editors: BTreeMap<InstKey, TextInput>,
    pub(crate) drag: Option<Drag>,
    /// The widget whose click fired THIS frame — painted pressed for the one
    /// frame between the event and the host's rebound state (see the paint
    /// walk's pressed-face bridge). Cleared at the top of every frame.
    pub(crate) clicked: Option<InstKey>,
    /// Last list-row press, for double-click activation.
    pub(crate) last_row_click: Option<(InstKey, u32, f64)>,
    /// Last observed bound selection per list, so a selection change (e.g.
    /// keyboard nav) can auto-scroll the enclosing scroll region.
    pub(crate) last_selected: BTreeMap<InstKey, i32>,
}

impl FrameState {
    pub fn new() -> FrameState {
        FrameState::default()
    }

    pub fn cursor(&self) -> (f32, f32) {
        self.cursor
    }

    pub fn focused(&self) -> Option<&InstKey> {
        self.focus.as_ref()
    }

    /// The active cursor-stack distribution gesture, for presentation-only
    /// host feedback. The ordered cells are the same de-duplicated hits that
    /// will be emitted in [`UiEvent::SlotDrag`] on release.
    pub fn slot_drag(&self) -> Option<(PointerButton, &[(String, u32)])> {
        match &self.drag {
            Some(Drag::Slots { button, slots, .. }) => Some((*button, slots)),
            _ => None,
        }
    }

    /// Focus a text input by key, creating its editor pre-loaded with `text`.
    pub fn focus_text_input(&mut self, key: InstKey, text: &str, max_chars: usize) {
        self.editors
            .entry(key.clone())
            .or_insert_with(|| TextInput::with_text(text, max_chars, self.now));
        if let Some(e) = self.editors.get_mut(&key) {
            e.focus(self.now);
        }
        self.focus = Some(key);
    }

    /// The live editor text for `id`, if one exists (focused now or earlier
    /// this session).
    pub fn editor_text(&self, id: &str) -> Option<&str> {
        self.editors
            .iter()
            .find(|(k, _)| k.id == id)
            .map(|(_, e)| e.text())
    }

    pub fn scroll_offset(&self, key: &InstKey) -> i32 {
        self.scroll.get(key).copied().unwrap_or(0)
    }

    pub fn set_scroll(&mut self, key: InstKey, offset: i32) {
        self.scroll.insert(key, offset);
    }

    /// Drop all transient interaction (screen change, GUI close).
    pub fn reset(&mut self) {
        self.active = None;
        self.focus = None;
        self.scroll.clear();
        self.editors.clear();
        self.drag = None;
        self.clicked = None;
        self.last_row_click = None;
        self.last_selected.clear();
    }
}

/// Builder-only forced state for the preview canvas: pretend the selected
/// node is hovered/pressed/disabled without real input.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewState {
    pub hover: Option<InstKey>,
    pub pressed: Option<InstKey>,
    pub focus: Option<InstKey>,
}
