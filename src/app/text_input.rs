use crate::gui::{shell_input_text_rect, SlotRect, TEXT_GLYPH_ADVANCE};

const CURSOR_STEADY_SECS: f64 = 0.45;
const CURSOR_BLINK_SECS: f64 = 0.5;

pub(crate) trait TextClipboard {
    fn get_text(&mut self) -> Option<String>;
    fn set_text(&mut self, text: &str) -> bool;
}

#[derive(Clone, Debug)]
pub(super) struct TextInput {
    text: String,
    cursor: usize,
    selection_anchor: Option<usize>,
    scroll: usize,
    max_chars: usize,
    last_activity: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct TextInputRender {
    pub text: String,
    pub cursor: usize,
    pub selection: Option<(usize, usize)>,
    pub show_cursor: bool,
}

impl TextInput {
    pub fn new(max_chars: usize) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            selection_anchor: None,
            scroll: 0,
            max_chars,
            last_activity: 0.0,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self, now: f64) {
        self.text.clear();
        self.cursor = 0;
        self.selection_anchor = None;
        self.scroll = 0;
        self.touch(now);
    }

    pub fn blur(&mut self) {
        self.selection_anchor = None;
    }

    pub fn focus(&mut self, now: f64) {
        self.touch(now);
    }

    pub fn insert_text(&mut self, text: &str, visible_chars: usize, now: f64) -> bool {
        let selected = self.selected_range();
        let selected_len = selected.map_or(0, |(start, end)| end - start);
        let available = self
            .max_chars
            .saturating_sub(self.len_chars().saturating_sub(selected_len));
        let accepted: String = text
            .chars()
            .filter(|&ch| is_text_char(ch))
            .take(available)
            .collect();
        if accepted.is_empty() {
            return false;
        }

        if let Some((start, end)) = selected {
            self.delete_range(start, end);
            self.cursor = start;
        }

        let at = byte_index(&self.text, self.cursor);
        self.text.insert_str(at, &accepted);
        self.cursor += accepted.chars().count();
        self.selection_anchor = None;
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        true
    }

    pub fn backspace(&mut self, visible_chars: usize, now: f64) -> bool {
        if self.delete_selection() {
            self.ensure_cursor_visible(visible_chars);
            self.touch(now);
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let end = self.cursor;
        let start = end - 1;
        self.delete_range(start, end);
        self.cursor = start;
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        true
    }

    pub fn delete_forward(&mut self, visible_chars: usize, now: f64) -> bool {
        if self.delete_selection() {
            self.ensure_cursor_visible(visible_chars);
            self.touch(now);
            return true;
        }
        if self.cursor >= self.len_chars() {
            return false;
        }
        self.delete_range(self.cursor, self.cursor + 1);
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        true
    }

    pub fn move_left(&mut self, extend_selection: bool, visible_chars: usize, now: f64) -> bool {
        let before = self.edit_state();
        if extend_selection {
            self.selection_anchor.get_or_insert(self.cursor);
            self.cursor = self.cursor.saturating_sub(1);
        } else if let Some((start, _end)) = self.selected_range() {
            self.cursor = start;
            self.selection_anchor = None;
        } else {
            self.cursor = self.cursor.saturating_sub(1);
            self.selection_anchor = None;
        }
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        self.edit_state() != before
    }

    pub fn move_right(&mut self, extend_selection: bool, visible_chars: usize, now: f64) -> bool {
        let before = self.edit_state();
        let len = self.len_chars();
        if extend_selection {
            self.selection_anchor.get_or_insert(self.cursor);
            self.cursor = (self.cursor + 1).min(len);
        } else if let Some((_start, end)) = self.selected_range() {
            self.cursor = end;
            self.selection_anchor = None;
        } else {
            self.cursor = (self.cursor + 1).min(len);
            self.selection_anchor = None;
        }
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        self.edit_state() != before
    }

    pub fn select_all(&mut self, visible_chars: usize, now: f64) -> bool {
        let len = self.len_chars();
        if len == 0 {
            self.cursor = 0;
            self.selection_anchor = None;
            self.scroll = 0;
            self.touch(now);
            return false;
        }
        self.selection_anchor = Some(0);
        self.cursor = len;
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        true
    }

    pub fn copy_selection(&self, clipboard: &mut dyn TextClipboard) -> bool {
        let Some((start, end)) = self.selected_range() else {
            return false;
        };
        clipboard.set_text(&self.slice_chars(start, end))
    }

    pub fn cut_selection(
        &mut self,
        clipboard: &mut dyn TextClipboard,
        visible_chars: usize,
        now: f64,
    ) -> bool {
        let Some((start, end)) = self.selected_range() else {
            return false;
        };
        let selected = self.slice_chars(start, end);
        if !clipboard.set_text(&selected) {
            return false;
        }
        self.delete_range(start, end);
        self.cursor = start;
        self.selection_anchor = None;
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        true
    }

    pub fn paste(
        &mut self,
        clipboard: &mut dyn TextClipboard,
        visible_chars: usize,
        now: f64,
    ) -> bool {
        let Some(text) = clipboard.get_text() else {
            return false;
        };
        self.insert_text(&text, visible_chars, now)
    }

    pub fn cursor_index_for_x(
        &self,
        x: f32,
        input_rect: SlotRect,
        scale: f32,
        visible_chars: usize,
    ) -> usize {
        if visible_chars == 0 {
            return self.scroll_for_view(visible_chars).min(self.len_chars());
        }
        let text_rect = shell_input_text_rect(input_rect, scale);
        let advance = TEXT_GLYPH_ADVANCE as f32 * scale.max(1.0);
        let offset = ((x - text_rect.x).max(0.0) / advance).round() as usize;
        let scroll = self.scroll_for_view(visible_chars);
        (scroll + offset)
            .min(scroll + visible_chars)
            .min(self.len_chars())
    }

    pub fn begin_drag(&mut self, index: usize, visible_chars: usize, now: f64) -> usize {
        self.cursor = index.min(self.len_chars());
        self.selection_anchor = Some(self.cursor);
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
        self.cursor
    }

    pub fn drag_to(&mut self, anchor: usize, index: usize, visible_chars: usize, now: f64) {
        self.selection_anchor = Some(anchor.min(self.len_chars()));
        self.cursor = index.min(self.len_chars());
        self.ensure_cursor_visible(visible_chars);
        self.touch(now);
    }

    pub fn render(&self, visible_chars: usize, active: bool, now: f64) -> TextInputRender {
        let scroll = self.scroll_for_view(visible_chars);
        let text: String = self.text.chars().skip(scroll).take(visible_chars).collect();
        let cursor = self.cursor.saturating_sub(scroll).min(visible_chars);
        let selection = self
            .selected_range()
            .and_then(|(start, end)| intersect_ranges(start, end, scroll, scroll + visible_chars))
            .map(|(start, end)| (start - scroll, end - scroll));
        TextInputRender {
            text,
            cursor,
            selection,
            show_cursor: active && self.cursor_visible(now),
        }
    }

    fn cursor_visible(&self, now: f64) -> bool {
        let idle = now - self.last_activity;
        if idle < CURSOR_STEADY_SECS {
            true
        } else {
            let phase = ((idle - CURSOR_STEADY_SECS) / CURSOR_BLINK_SECS).floor() as u64;
            phase % 2 == 0
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selected_range() else {
            return false;
        };
        self.delete_range(start, end);
        self.cursor = start;
        self.selection_anchor = None;
        true
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        let start_b = byte_index(&self.text, start);
        let end_b = byte_index(&self.text, end);
        self.text.replace_range(start_b..end_b, "");
    }

    fn selected_range(&self) -> Option<(usize, usize)> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    fn slice_chars(&self, start: usize, end: usize) -> String {
        self.text
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect()
    }

    fn ensure_cursor_visible(&mut self, visible_chars: usize) {
        let len = self.len_chars();
        if visible_chars == 0 {
            self.scroll = self.cursor.min(len);
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor > self.scroll + visible_chars {
            self.scroll = self.cursor - visible_chars;
        }
        self.scroll = self.scroll.min(len.saturating_sub(visible_chars));
    }

    fn scroll_for_view(&self, visible_chars: usize) -> usize {
        self.scroll
            .min(self.len_chars().saturating_sub(visible_chars))
    }

    fn len_chars(&self) -> usize {
        self.text.chars().count()
    }

    fn touch(&mut self, now: f64) {
        self.last_activity = now;
    }

    fn edit_state(&self) -> (usize, Option<usize>, usize) {
        (self.cursor, self.selection_anchor, self.scroll)
    }
}

fn is_text_char(ch: char) -> bool {
    ch.is_ascii_graphic() || ch == ' '
}

fn byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn intersect_ranges(
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
) -> Option<(usize, usize)> {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    (start < end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryClipboard {
        text: Option<String>,
    }

    impl TextClipboard for MemoryClipboard {
        fn get_text(&mut self) -> Option<String> {
            self.text.clone()
        }

        fn set_text(&mut self, text: &str) -> bool {
            self.text = Some(text.to_string());
            true
        }
    }

    #[test]
    fn inserts_supported_symbols_and_filters_controls() {
        let mut input = TextInput::new(48);

        assert!(input.insert_text("World $#@!^{}~\n", 12, 0.0));

        assert_eq!(input.text(), "World $#@!^{}~");
    }

    #[test]
    fn arrow_without_shift_collapses_selection() {
        let mut input = TextInput::new(48);
        input.insert_text("abcdef", 6, 0.0);
        input.select_all(6, 0.1);

        assert!(input.move_left(false, 6, 0.2));
        assert_eq!(input.render(6, true, 0.2).cursor, 0);
        assert_eq!(input.render(6, true, 0.2).selection, None);

        input.select_all(6, 0.3);
        assert!(input.move_right(false, 6, 0.4));
        assert_eq!(input.render(6, true, 0.4).cursor, 6);
        assert_eq!(input.render(6, true, 0.4).selection, None);
    }

    #[test]
    fn shift_arrow_expands_selection() {
        let mut input = TextInput::new(48);
        input.insert_text("abcdef", 6, 0.0);
        input.move_left(false, 6, 0.1);
        input.move_left(false, 6, 0.2);

        input.move_left(true, 6, 0.3);
        input.move_left(true, 6, 0.4);

        let view = input.render(6, true, 0.4);
        assert_eq!(view.cursor, 2);
        assert_eq!(view.selection, Some((2, 4)));
    }

    #[test]
    fn insertion_replaces_selected_text() {
        let mut input = TextInput::new(48);
        input.insert_text("abcdef", 6, 0.0);
        input.move_left(false, 6, 0.1);
        input.move_left(false, 6, 0.2);
        input.move_left(true, 6, 0.3);
        input.move_left(true, 6, 0.4);

        assert!(input.insert_text("XY", 6, 0.5));

        assert_eq!(input.text(), "abXYef");
        assert_eq!(input.render(6, true, 0.5).cursor, 4);
        assert_eq!(input.render(6, true, 0.5).selection, None);
    }

    #[test]
    fn scroll_follows_cursor_in_both_directions() {
        let mut input = TextInput::new(48);
        input.insert_text("abcdefghij", 4, 0.0);

        let end = input.render(4, true, 0.0);
        assert_eq!(end.text, "ghij");
        assert_eq!(end.cursor, 4);

        for i in 0..7 {
            input.move_left(false, 4, 0.1 + i as f64);
        }
        let middle = input.render(4, true, 1.0);
        assert_eq!(middle.text, "defg");
        assert_eq!(middle.cursor, 0);

        input.move_left(false, 4, 1.1);
        let left = input.render(4, true, 1.1);
        assert_eq!(left.text, "cdef");
        assert_eq!(left.cursor, 0);
    }

    #[test]
    fn copy_cut_and_paste_use_selected_text() {
        let mut input = TextInput::new(48);
        let mut clipboard = MemoryClipboard::default();
        input.insert_text("abcdef", 6, 0.0);
        input.move_left(false, 6, 0.1);
        input.move_left(false, 6, 0.2);
        input.move_left(true, 6, 0.3);
        input.move_left(true, 6, 0.4);

        assert!(input.copy_selection(&mut clipboard));
        assert_eq!(clipboard.text.as_deref(), Some("cd"));

        assert!(input.cut_selection(&mut clipboard, 6, 0.5));
        assert_eq!(input.text(), "abef");

        assert!(input.paste(&mut clipboard, 6, 0.6));
        assert_eq!(input.text(), "abcdef");
    }

    #[test]
    fn drag_selects_between_mouse_positions() {
        let mut input = TextInput::new(48);
        input.insert_text("abcdef", 6, 0.0);
        let rect = SlotRect {
            x: 10.0,
            y: 0.0,
            w: 200.0,
            h: 20.0,
        };
        let start = input.cursor_index_for_x(21.0, rect, 1.0, 6);
        let end = input.cursor_index_for_x(39.0, rect, 1.0, 6);
        let anchor = input.begin_drag(start, 6, 0.1);

        input.drag_to(anchor, end, 6, 0.2);

        assert_eq!(input.render(6, true, 0.2).selection, Some((1, 4)));
    }

    #[test]
    fn cursor_blinks_only_after_idle_period() {
        let mut input = TextInput::new(48);
        input.focus(1.0);

        assert!(input.render(8, true, 1.2).show_cursor);
        assert!(input.render(8, true, 1.5).show_cursor);
        assert!(!input.render(8, true, 2.0).show_cursor);
    }
}
