//! Host input events, per-GUI ephemeral widget state, and resolved UI events.
//!
//! [`FrameState`] is everything that must persist across frames while a GUI
//! is open but must NEVER cross into game/tick state: hover, press, focus,
//! scroll offsets, text editors, drags. The host owns one per open GUI and
//! drops it on close. The only artifacts that may reach a deterministic tick
//! are the [`UiEvent`]s the host chooses to latch.

use crate::text_edit::TextInput;
use crate::tree::{InstKey, InstTree};
use std::collections::{BTreeMap, HashSet};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// Semantic (host-keymapped) keys: the host translates ctrl+C → `Copy` etc.,
/// and passes Shift/Ctrl alongside movement/edit keys, so petramond-ui never
/// owns a keymap.
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
    /// A function key, `F(5)` = F5.
    F(u8),
    /// A letter or digit, always lowercase: pressed with Ctrl (the
    /// clipboard chords aside, `ctrl: true`), or typed with no text input
    /// focused to take it (`ctrl: false`).
    Char(char),
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
        ctrl: bool,
    },
    Char {
        ch: char,
    },
    /// Pointer/keyboard focus left the window: release presses and drags.
    Blur,
    /// The modifier keys held changed (surface events report them).
    Modifiers(Mods),
}

/// The modifier keys held, as surface pointer and wheel events report them.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
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
    /// A checkbox or toggle was pressed. `on` is the value the widget's bound
    /// state would take; `button` is the pointer button that pressed it, so a
    /// host applies the same primary-only policy it applies to
    /// [`Click`](Self::Click) instead of flipping under a right-click.
    Toggle {
        id: String,
        item: Option<u32>,
        on: bool,
        button: PointerButton,
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
    /// A text input lost focus — by a click elsewhere, Tab, Escape or the
    /// host moving focus: the moment a field typed into is "left".
    Blur {
        id: String,
        item: Option<u32>,
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
    /// The pointer over an interactive `canvas`/`viewport`, in logical px
    /// local to its rect: presses (with the click count of a streak), moves
    /// (at most one per frame; with `button` while one is held, since a
    /// press captures the pointer until its release), and `Leave` when an
    /// uncaptured pointer leaves it.
    SurfacePointer {
        id: String,
        item: Option<u32>,
        phase: PointerPhase,
        x: f32,
        y: f32,
        button: Option<PointerButton>,
        mods: Mods,
        clicks: u8,
    },
    /// Wheel travel over an interactive surface (logical px, positive =
    /// content up) — the surface's, never an enclosing scroll's.
    SurfaceScroll {
        id: String,
        item: Option<u32>,
        x: f32,
        y: f32,
        delta: i32,
        mods: Mods,
    },
    /// An interactive surface's solved size, on its first frame and every
    /// time the layout changes it (logical px).
    SurfaceSize {
        id: String,
        item: Option<u32>,
        w: i32,
        h: i32,
    },
    /// Double-click / Enter on a list row.
    ListActivate {
        id: String,
        index: u32,
    },
    /// A tab of a `tab_bar` was pressed (fires on pointer down). The host
    /// rebinds `selected` to accept the change.
    TabSelect {
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
        ctrl: bool,
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
    Surface {
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
    /// The pointer left a surface it was hovering (surfaces only).
    Leave,
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
    /// The named widget under the cursor on the LAST frame — the hover
    /// anchor a `tooltip` node's `hover` property matches against. One frame
    /// old, the same contract as hover-revealed list content.
    pub(crate) hover_widget: Option<InstKey>,
    /// The text input focused at the end of the last frame, so a focus that
    /// moved by any path reports the input it left.
    pub(crate) last_focus: Option<InstKey>,
    /// The instance of each widget id last pressed — what an `anchor_to`
    /// popup outside a list template sits under.
    pub(crate) last_pressed: BTreeMap<String, InstKey>,
    /// A host's request to focus a text input, carried out by the next frame
    /// (which knows the input's bound text and limits).
    pub(crate) pending_focus: Option<InstKey>,
    pub(crate) mods: Mods,
    /// The surface a moved pointer last reported on this frame, reported
    /// once at the frame's end (or before a release).
    pub(crate) surface_move: Option<(InstKey, f32, f32, Option<PointerButton>)>,
    /// The surface the pointer hovered at the end of the last frame.
    pub(crate) surface_hover: Option<InstKey>,
    /// The last surface press: which, when, where, and its streak count.
    pub(crate) surface_press: Option<(InstKey, f64, (f32, f32), u8)>,
    /// Each interactive surface's last reported size.
    pub(crate) surface_sizes: BTreeMap<InstKey, (i32, i32)>,
    /// Last frame's expanded arena and layout (see `crate::runtime::cache`).
    pub(crate) cache: crate::runtime::cache::FrameCache,
}

impl FrameState {
    pub fn new() -> FrameState {
        FrameState::default()
    }

    pub fn cursor(&self) -> (f32, f32) {
        self.cursor
    }

    /// What the frame cache did on the last frame: how much of the arena was
    /// expanded afresh versus carried over, and whether layout was reused.
    pub fn cache_stats(&self) -> crate::runtime::CacheStats {
        self.cache.stats
    }

    pub fn focused(&self) -> Option<&InstKey> {
        self.focus.as_ref()
    }

    /// Focus text input `key` (caret at the end) on the next frame, if it is
    /// then an enabled text input; otherwise the request lapses.
    pub fn request_focus(&mut self, key: InstKey) {
        self.pending_focus = Some(key);
    }

    /// The named widget under the cursor as of the last frame.
    pub fn hover_widget(&self) -> Option<&InstKey> {
        self.hover_widget.as_ref()
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

    /// Forget the per-widget state of every instance `tree` no longer holds
    /// — a list row scrolled out of the data, a tab's inputs after the tab
    /// closed — so a long-lived screen's maps track what is on it, not
    /// everything it ever showed. The focused editor survives: focus is what
    /// a host clears, not a frame's expansion.
    pub(crate) fn retain_live(&mut self, tree: &InstTree<'_>) {
        let live: HashSet<&InstKey> = tree.insts.iter().filter_map(|i| i.key.as_ref()).collect();
        let focus = self.focus.as_ref();
        self.scroll.retain(|key, _| live.contains(key));
        self.last_selected.retain(|key, _| live.contains(key));
        self.last_pressed.retain(|_, key| live.contains(key));
        self.surface_sizes.retain(|key, _| live.contains(key));
        self.editors
            .retain(|key, _| live.contains(key) || focus == Some(key));
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
        self.hover_widget = None;
        self.last_focus = None;
        self.last_pressed.clear();
        self.pending_focus = None;
        self.surface_move = None;
        self.surface_hover = None;
        self.surface_press = None;
        self.surface_sizes.clear();
        self.cache = Default::default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use crate::paint_walk::NoImages;
    use crate::runtime::{FrameArgs, FrameOutput, UiRuntime};
    use crate::state::{UiMap, UiState, UiValue};
    use crate::theme::Theme;
    use std::sync::Arc;

    fn rows(n: usize) -> UiValue {
        UiValue::List(Arc::new((0..n).map(|_| UiMap::new()).collect()))
    }

    fn frame(rt: &UiRuntime, fs: &mut FrameState, state: &UiState) {
        rt.frame(
            FrameArgs {
                screen: (400, 400),
                scale: 1,
                now: 0.0,
                state,
                input: &[],
                clipboard: None,
                images: &NoImages,
                dim: None,
                preview: None,
            },
            fs,
            &mut FrameOutput::default(),
        );
    }

    /// Per-row widget state goes when its row does, instead of piling up for
    /// every row a long-lived screen ever showed.
    #[test]
    fn widget_state_of_vanished_instances_is_forgotten() {
        let doc = Document::from_json(
            r#"{ "format": 1, "kind": "petramond:t", "class": "screen",
                 "root": { "type": "column", "children": [
                   { "type": "list", "id": "rows", "bind": { "items": "rows" }, "children": [
                     { "type": "column", "children": [
                       { "type": "scroll", "id": "sc", "layout": { "h": 10 } },
                       { "type": "text_input", "id": "note" }
                     ] }
                   ] }
                 ] } }"#,
        )
        .unwrap();
        let rt = UiRuntime::new(Arc::new(doc), Arc::new(Theme::placeholder()));
        let mut fs = FrameState::new();
        let mut state = UiState::new();
        state.set("rows", rows(3));
        frame(&rt, &mut fs, &state);
        let key = |id: &str, item| InstKey {
            id: id.into(),
            item: Some(item),
        };
        for item in 0..3 {
            fs.set_scroll(key("sc", item), 1);
            fs.focus_text_input(key("note", item), "", 8);
        }
        assert_eq!(fs.focused(), Some(&key("note", 2)));

        state.set("rows", rows(1));
        frame(&rt, &mut fs, &state);
        let scrolled: Vec<&InstKey> = fs.scroll.keys().collect();
        assert_eq!(scrolled, [&key("sc", 0)]);
        let edited: Vec<&InstKey> = fs.editors.keys().collect();
        assert_eq!(
            edited,
            [&key("note", 0), &key("note", 2)],
            "the focused editor survives its row"
        );
    }
}
