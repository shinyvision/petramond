use crate::text_edit::TextInput;
use crate::tree::{InstKey, InstTree};
use std::collections::{BTreeMap, HashSet};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

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
    F(u8),
    Char(char),
}

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
        slot_drag: bool,
    },
    PointerUp {
        x: f32,
        y: f32,
        button: PointerButton,
    },
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
    Blur,
    Modifiers(Mods),
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

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
        button: PointerButton,
    },
    SliderChange {
        id: String,
        item: Option<u32>,
        value: f32,
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
    Blur {
        id: String,
        item: Option<u32>,
    },
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
    SurfaceScroll {
        id: String,
        item: Option<u32>,
        x: f32,
        y: f32,
        delta: i32,
        mods: Mods,
    },
    SurfaceSize {
        id: String,
        item: Option<u32>,
        w: i32,
        h: i32,
    },
    ListActivate {
        id: String,
        index: u32,
    },
    TabSelect {
        id: String,
        index: u32,
    },
    SlotClick {
        role: String,
        index: u32,
        button: PointerButton,
        shift: bool,
    },
    SlotDrag {
        slots: Vec<(String, u32)>,
        button: PointerButton,
    },
    ClickOutside {
        button: PointerButton,
    },
    Key {
        key: NavKey,
        shift: bool,
        ctrl: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Drag {
    Slider {
        key: InstKey,
    },
    ScrollThumb {
        key: InstKey,
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
        slots: Vec<(String, u32)>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Leave,
}

#[derive(Debug, Default)]
pub struct FrameState {
    pub now: f64,
    pub(crate) cursor: (f32, f32),
    pub(crate) active: Option<(InstKey, PointerButton)>,
    pub(crate) focus: Option<InstKey>,
    pub(crate) scroll: BTreeMap<InstKey, i32>,
    pub(crate) editors: BTreeMap<InstKey, TextInput>,
    pub(crate) drag: Option<Drag>,
    pub(crate) clicked: Option<InstKey>,
    pub(crate) last_row_click: Option<(InstKey, u32, f64)>,
    pub(crate) last_selected: BTreeMap<InstKey, i32>,
    pub(crate) hover_widget: Option<InstKey>,
    pub(crate) last_focus: Option<InstKey>,
    pub(crate) last_pressed: BTreeMap<String, InstKey>,
    pub(crate) pending_focus: Option<InstKey>,
    pub(crate) mods: Mods,
    pub(crate) surface_move: Option<(InstKey, f32, f32, Option<PointerButton>)>,
    pub(crate) surface_hover: Option<InstKey>,
    pub(crate) surface_press: Option<(InstKey, f64, (f32, f32), u8)>,
    pub(crate) surface_sizes: BTreeMap<InstKey, (i32, i32)>,
    pub(crate) cache: crate::runtime::cache::FrameCache,
}

impl FrameState {
    pub fn new() -> FrameState {
        FrameState::default()
    }

    pub fn cursor(&self) -> (f32, f32) {
        self.cursor
    }

    pub fn cache_stats(&self) -> crate::runtime::CacheStats {
        self.cache.stats
    }

    pub fn focused(&self) -> Option<&InstKey> {
        self.focus.as_ref()
    }

    pub fn request_focus(&mut self, key: InstKey) {
        self.pending_focus = Some(key);
    }

    pub fn hover_widget(&self) -> Option<&InstKey> {
        self.hover_widget.as_ref()
    }

    pub fn slot_drag(&self) -> Option<(PointerButton, &[(String, u32)])> {
        match &self.drag {
            Some(Drag::Slots { button, slots, .. }) => Some((*button, slots)),
            _ => None,
        }
    }

    pub fn focus_text_input(&mut self, key: InstKey, text: &str, max_chars: usize) {
        self.editors
            .entry(key.clone())
            .or_insert_with(|| TextInput::with_text(text, max_chars, self.now));
        if let Some(e) = self.editors.get_mut(&key) {
            e.focus(self.now);
        }
        self.focus = Some(key);
    }

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
