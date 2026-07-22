//! Client-side chat UI state and drawing.
//!
//! The server owns accepted/formatted chat lines. This module owns only local
//! presentation: history retention, draft editing, scrolling, and fade timing.

use std::collections::VecDeque;

use crate::net::protocol::{ChatColor, ChatLine, ChatSpan, MAX_CHAT_CHARS};

const MAX_HISTORY: usize = 256;
const MAX_SENT: usize = 64;
const PASSIVE_SECS: f64 = 10.0;
const FADE_SECS: f64 = 1.0;
const PASSIVE_MAX_LINES: usize = 6;
const CHAT_W: i32 = 360;
const OPEN_HISTORY_H: i32 = 118;
const PAD: i32 = 4;
const INPUT_PREFIX: &str = "> ";

#[derive(Clone)]
struct TimedLine {
    line: ChatLine,
    received_at: f64,
}

#[derive(Copy, Clone)]
struct InputLayout {
    scale: i32,
    input: petramond_ui::RectI,
    text_x: i32,
    visible_chars: usize,
}

pub(super) struct ChatUi {
    history: VecDeque<TimedLine>,
    sent: VecDeque<String>,
    history_cursor: Option<usize>,
    draft_backup: Option<String>,
    editor: petramond_ui::TextInput,
    scroll_lines: usize,
    history_chars: usize,
    input_layout: Option<InputLayout>,
    drag_anchor: Option<usize>,
}

impl Default for ChatUi {
    fn default() -> Self {
        Self {
            history: VecDeque::new(),
            sent: VecDeque::new(),
            history_cursor: None,
            draft_backup: None,
            editor: petramond_ui::TextInput::new(MAX_CHAT_CHARS),
            scroll_lines: 0,
            history_chars: chars_for_width(CHAT_W - PAD * 2),
            input_layout: None,
            drag_anchor: None,
        }
    }
}

impl ChatUi {
    pub(super) fn push(&mut self, line: ChatLine, now: f64) {
        if self.history.len() == MAX_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(TimedLine {
            line,
            received_at: now,
        });
        self.scroll_lines = self.scroll_lines.min(self.max_scroll_lines());
    }

    pub(super) fn insert_text(&mut self, text: &str, now: f64) {
        self.history_cursor = None;
        self.draft_backup = None;
        self.editor
            .insert_text(text, self.input_visible_chars(), now);
    }

    pub(super) fn edit_key(
        &mut self,
        key: petramond_ui::NavKey,
        shift: bool,
        ctrl: bool,
        clipboard: Option<&mut dyn petramond_ui::TextClipboard>,
        now: f64,
    ) {
        let visible = self.input_visible_chars();
        match (key, ctrl) {
            (petramond_ui::NavKey::Left, true) => {
                self.editor.move_word_left(shift, visible, now);
            }
            (petramond_ui::NavKey::Right, true) => {
                self.editor.move_word_right(shift, visible, now);
            }
            (petramond_ui::NavKey::Left, false) => {
                self.editor.move_left(shift, visible, now);
            }
            (petramond_ui::NavKey::Right, false) => {
                self.editor.move_right(shift, visible, now);
            }
            (petramond_ui::NavKey::Home, _) => self.editor.move_home(shift, visible, now),
            (petramond_ui::NavKey::End, _) => self.editor.move_end(shift, visible, now),
            (petramond_ui::NavKey::Up, _) => self.history_prev(visible, now),
            (petramond_ui::NavKey::Down, _) => self.history_next(visible, now),
            (petramond_ui::NavKey::Backspace, true) => {
                self.editor.backspace_word(visible, now);
            }
            (petramond_ui::NavKey::Backspace, false) => {
                self.editor.backspace(visible, now);
            }
            (petramond_ui::NavKey::Delete, true) => {
                self.editor.delete_word_forward(visible, now);
            }
            (petramond_ui::NavKey::Delete, false) => {
                self.editor.delete_forward(visible, now);
            }
            (petramond_ui::NavKey::SelectAll, _) => {
                self.editor.select_all(visible, now);
            }
            (petramond_ui::NavKey::Copy, _) => {
                if let Some(cb) = clipboard {
                    self.editor.copy_selection(cb);
                }
            }
            (petramond_ui::NavKey::Cut, _) => {
                if let Some(cb) = clipboard {
                    self.editor.cut_selection(cb, visible, now);
                }
            }
            (petramond_ui::NavKey::Paste, _) => {
                if let Some(cb) = clipboard {
                    self.editor.paste(cb, visible, now);
                }
            }
            _ => {}
        }
    }

    pub(super) fn submit_or_close(&mut self, now: f64) -> Option<String> {
        let text = self.editor.text().trim().to_owned();
        if !text.is_empty() {
            if self.sent.len() == MAX_SENT {
                self.sent.pop_front();
            }
            self.sent.push_back(text.clone());
        }
        self.clear_draft(now);
        (!text.is_empty()).then_some(text)
    }

    pub(super) fn clear_draft(&mut self, now: f64) {
        self.editor.clear(now);
        self.editor.focus(now);
        self.drag_anchor = None;
        self.history_cursor = None;
        self.draft_backup = None;
    }

    fn history_prev(&mut self, visible: usize, now: f64) {
        if self.sent.is_empty() {
            return;
        }
        let next = match self.history_cursor {
            None => {
                self.draft_backup = Some(self.editor.text().to_owned());
                self.sent.len() - 1
            }
            Some(i) => i.saturating_sub(1),
        };
        self.history_cursor = Some(next);
        let text = self.sent[next].clone();
        self.set_draft(&text, visible, now);
    }

    fn history_next(&mut self, visible: usize, now: f64) {
        let Some(i) = self.history_cursor else {
            return;
        };
        if i + 1 < self.sent.len() {
            self.history_cursor = Some(i + 1);
            let text = self.sent[i + 1].clone();
            self.set_draft(&text, visible, now);
        } else {
            self.history_cursor = None;
            let draft = self.draft_backup.take().unwrap_or_default();
            self.set_draft(&draft, visible, now);
        }
    }

    fn set_draft(&mut self, text: &str, visible: usize, now: f64) {
        self.editor.clear(now);
        self.editor.insert_text(text, visible, now);
        self.drag_anchor = None;
    }

    pub(super) fn pointer_down(&mut self, x: f32, y: f32, now: f64) {
        let Some(layout) = self.input_layout else {
            return;
        };
        let lx = x / layout.scale as f32;
        let ly = y / layout.scale as f32;
        if !contains(layout.input, lx, ly) {
            self.drag_anchor = None;
            return;
        }
        let idx = self.editor.cursor_index_for_x(
            lx - layout.text_x as f32,
            petramond_ui::text::ADVANCE as f32,
            layout.visible_chars,
        );
        self.drag_anchor = Some(self.editor.begin_drag(idx, layout.visible_chars, now));
    }

    pub(super) fn pointer_move(&mut self, x: f32, _y: f32, now: f64) {
        let (Some(anchor), Some(layout)) = (self.drag_anchor, self.input_layout) else {
            return;
        };
        let lx = x / layout.scale as f32;
        let idx = self.editor.cursor_index_for_x(
            lx - layout.text_x as f32,
            petramond_ui::text::ADVANCE as f32,
            layout.visible_chars,
        );
        self.editor.drag_to(anchor, idx, layout.visible_chars, now);
    }

    pub(super) fn pointer_up(&mut self) {
        self.drag_anchor = None;
    }

    pub(super) fn scroll(&mut self, wheel_delta: f32) {
        if wheel_delta < 0.0 {
            self.scroll_lines = self.scroll_lines.saturating_add(3);
        } else if wheel_delta > 0.0 {
            self.scroll_lines = self.scroll_lines.saturating_sub(3);
        }
        self.scroll_lines = self.scroll_lines.min(self.max_scroll_lines());
    }

    pub(super) fn draw(
        &mut self,
        draw: &mut petramond_ui::DrawList,
        screen: (u32, u32),
        open: bool,
        now: f64,
    ) {
        let scale = crate::gui::gui_scale(screen) as i32;
        let mut p = petramond_ui::Painter { list: draw, scale };
        if open {
            self.draw_open(&mut p, screen, now);
        } else {
            self.input_layout = None;
            self.draw_passive(&mut p, screen, now);
        }
    }

    fn draw_passive(&mut self, p: &mut petramond_ui::Painter<'_>, screen: (u32, u32), now: f64) {
        let logical = logical_screen(screen);
        let w = CHAT_W.min(logical.0 - 16).max(120);
        let max_chars = chars_for_width(w - PAD * 2);
        self.history_chars = max_chars;
        let mut y = logical.1 - 64;
        let mut drawn = 0usize;
        for line in self.history.iter().rev() {
            let age = now - line.received_at;
            let alpha = if age <= PASSIVE_SECS {
                1.0
            } else if age <= PASSIVE_SECS + FADE_SECS {
                1.0 - ((age - PASSIVE_SECS) / FADE_SECS) as f32
            } else {
                continue;
            };
            let wrapped = wrap_spans(&line.line.spans, max_chars);
            for visual in wrapped.iter().rev() {
                if drawn >= PASSIVE_MAX_LINES || y < 4 {
                    return;
                }
                y -= petramond_ui::text::LINE_ADVANCE + 2;
                let backing = petramond_ui::RectI {
                    x: 8,
                    y: y - 1,
                    w,
                    h: petramond_ui::text::GLYPH_H + 2,
                };
                p.solid(backing, [0.0, 0.0, 0.0, 0.38 * alpha], None);
                draw_visual_line(p, 8 + PAD, y, visual, alpha, Some(backing));
                drawn += 1;
            }
        }
    }

    fn draw_open(&mut self, p: &mut petramond_ui::Painter<'_>, screen: (u32, u32), now: f64) {
        let logical = logical_screen(screen);
        let w = CHAT_W.min(logical.0 - 16).max(120);
        let x = 8;
        let input_h = 16;
        let input_y = logical.1 - 52;
        let history_y = input_y - OPEN_HISTORY_H - 4;
        let history = petramond_ui::RectI {
            x,
            y: history_y,
            w,
            h: OPEN_HISTORY_H,
        };
        p.solid(history, [0.0, 0.0, 0.0, 0.58], None);

        let max_chars = chars_for_width(w - PAD * 2);
        self.history_chars = max_chars;
        let lines = self.visual_history(max_chars);
        let visible = open_history_visible_lines();
        let end = lines
            .len()
            .saturating_sub(self.scroll_lines.min(lines.len()));
        let start = end.saturating_sub(visible);
        let visible_count = end.saturating_sub(start);
        let mut y = open_history_first_line_y(history, visible_count);
        for visual in &lines[start..end] {
            draw_visual_line(p, x + PAD, y, visual, 1.0, Some(history));
            y += petramond_ui::text::LINE_ADVANCE;
        }

        let input = petramond_ui::RectI {
            x,
            y: input_y,
            w,
            h: input_h,
        };
        p.solid(input, [0.0, 0.0, 0.0, 0.72], None);
        let prefix_w = INPUT_PREFIX.chars().count() as i32 * petramond_ui::text::ADVANCE;
        let text_x = x + PAD + prefix_w;
        let text_y = input_y + (input_h - petramond_ui::text::GLYPH_H) / 2;
        let visible = chars_for_width(w - PAD * 2 - prefix_w);
        self.input_layout = Some(InputLayout {
            scale: p.scale,
            input,
            text_x,
            visible_chars: visible,
        });
        p.text(
            INPUT_PREFIX,
            x + PAD,
            text_y,
            [1.0, 1.0, 1.0, 1.0],
            Some(input),
        );
        let view = self.editor.render(visible, true, now);
        p.text_input_view(
            &view,
            text_x,
            text_y,
            [1.0, 1.0, 1.0, 1.0],
            [0.24, 0.46, 0.92, 0.7],
            Some(input),
        );
    }

    fn visual_history(&self, max_chars: usize) -> Vec<Vec<ColoredText>> {
        self.history
            .iter()
            .flat_map(|line| wrap_spans(&line.line.spans, max_chars))
            .collect()
    }

    fn max_scroll_lines(&self) -> usize {
        let lines = self.visual_history(self.history_chars);
        let visible = open_history_visible_lines();
        lines.len().saturating_sub(visible)
    }

    fn input_visible_chars(&self) -> usize {
        self.input_layout
            .map(|layout| layout.visible_chars)
            .unwrap_or_else(|| {
                let prefix_w = INPUT_PREFIX.chars().count() as i32 * petramond_ui::text::ADVANCE;
                chars_for_width(CHAT_W - PAD * 2 - prefix_w)
            })
    }
}

#[derive(Clone)]
struct ColoredGlyph {
    ch: char,
    fg: ChatColor,
}

#[derive(Clone)]
struct ColoredText {
    fg: ChatColor,
    text: String,
}

fn logical_screen(screen: (u32, u32)) -> (i32, i32) {
    let scale = crate::gui::gui_scale(screen) as i32;
    (
        (screen.0 as i32 / scale).max(1),
        (screen.1 as i32 / scale).max(1),
    )
}

fn chars_for_width(w: i32) -> usize {
    (w / petramond_ui::text::ADVANCE).max(1) as usize
}

fn contains(r: petramond_ui::RectI, x: f32, y: f32) -> bool {
    x >= r.x as f32 && y >= r.y as f32 && x < (r.x + r.w) as f32 && y < (r.y + r.h) as f32
}

fn open_history_visible_lines() -> usize {
    let content_h = OPEN_HISTORY_H.saturating_sub(PAD * 2);
    if content_h <= petramond_ui::text::GLYPH_H {
        return 1;
    }
    ((content_h - petramond_ui::text::GLYPH_H) / petramond_ui::text::LINE_ADVANCE + 1) as usize
}

fn open_history_first_line_y(history: petramond_ui::RectI, line_count: usize) -> i32 {
    let last_y = history.y + history.h - PAD - petramond_ui::text::GLYPH_H;
    last_y - line_count.saturating_sub(1) as i32 * petramond_ui::text::LINE_ADVANCE
}

fn wrap_spans(spans: &[ChatSpan], max_chars: usize) -> Vec<Vec<ColoredText>> {
    let mut glyphs = Vec::new();
    for span in spans {
        glyphs.extend(span.text.chars().map(|ch| ColoredGlyph { ch, fg: span.fg }));
    }
    if glyphs.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut line = Vec::new();
    for glyph in glyphs {
        if glyph.ch == '\n' {
            out.push(compact_line(&line));
            line.clear();
            continue;
        }
        line.push(glyph);
        if line.len() >= max_chars {
            let split = line
                .iter()
                .rposition(|g| g.ch == ' ')
                .filter(|&i| i > 0)
                .unwrap_or(line.len());
            let remainder = if split < line.len() {
                line.split_off(split + 1)
            } else {
                Vec::new()
            };
            let drawn = if split < line.len() {
                &line[..split]
            } else {
                &line[..]
            };
            out.push(compact_line(drawn));
            line = remainder;
        }
    }
    if !line.is_empty() {
        out.push(compact_line(&line));
    }
    out
}

fn compact_line(glyphs: &[ColoredGlyph]) -> Vec<ColoredText> {
    let mut out: Vec<ColoredText> = Vec::new();
    for glyph in glyphs {
        if let Some(last) = out.last_mut().filter(|run| run.fg == glyph.fg) {
            last.text.push(glyph.ch);
        } else {
            out.push(ColoredText {
                fg: glyph.fg,
                text: glyph.ch.to_string(),
            });
        }
    }
    out
}

fn draw_visual_line(
    p: &mut petramond_ui::Painter<'_>,
    x: i32,
    y: i32,
    line: &[ColoredText],
    alpha: f32,
    clip: Option<petramond_ui::RectI>,
) {
    let mut cx = x;
    for run in line {
        p.text(&run.text, cx, y, color(run.fg, alpha), clip);
        cx += run.text.chars().count() as i32 * petramond_ui::text::ADVANCE;
    }
}

fn color(fg: ChatColor, alpha: f32) -> [f32; 4] {
    let [r, g, b] = match fg {
        ChatColor::White => [1.0, 1.0, 1.0],
        ChatColor::Red => [1.0, 0.25, 0.22],
        ChatColor::Yellow => [1.0, 0.88, 0.18],
        ChatColor::Blue => [0.42, 0.62, 1.0],
        ChatColor::Cyan => [0.35, 0.95, 1.0],
    };
    [r, g, b, alpha.clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(seq: u64, text: &str) -> ChatLine {
        ChatLine {
            seq,
            spans: vec![ChatSpan {
                fg: ChatColor::White,
                text: text.to_owned(),
            }],
        }
    }

    #[test]
    fn history_is_capped_to_client_side_limit() {
        let mut chat = ChatUi::default();
        for i in 0..300 {
            chat.push(line(i, "x"), i as f64);
        }
        assert_eq!(chat.history.len(), MAX_HISTORY);
        assert_eq!(chat.history.front().unwrap().line.seq, 44);
    }

    #[test]
    fn open_history_visible_lines_fit_inside_panel() {
        let panel = petramond_ui::RectI {
            x: 8,
            y: 20,
            w: CHAT_W,
            h: OPEN_HISTORY_H,
        };
        let visible = open_history_visible_lines();
        let first_y = open_history_first_line_y(panel, visible);
        let last_y = first_y + visible.saturating_sub(1) as i32 * petramond_ui::text::LINE_ADVANCE;

        assert!(first_y >= panel.y + PAD);
        assert!(last_y + petramond_ui::text::GLYPH_H <= panel.y + panel.h - PAD);
    }

    fn send(chat: &mut ChatUi, text: &str) {
        chat.insert_text(text, 0.0);
        assert_eq!(chat.submit_or_close(0.0).as_deref(), Some(text));
    }

    fn press(chat: &mut ChatUi, key: petramond_ui::NavKey) {
        chat.edit_key(key, false, false, None, 0.0);
    }

    #[test]
    fn up_recalls_sent_lines_newest_first_down_walks_back() {
        let mut chat = ChatUi::default();
        send(&mut chat, "hello");
        send(&mut chat, "/time set day");

        press(&mut chat, petramond_ui::NavKey::Up);
        assert_eq!(chat.editor.text(), "/time set day");
        press(&mut chat, petramond_ui::NavKey::Up);
        assert_eq!(chat.editor.text(), "hello");
        // Already at the oldest entry: stays put.
        press(&mut chat, petramond_ui::NavKey::Up);
        assert_eq!(chat.editor.text(), "hello");
        press(&mut chat, petramond_ui::NavKey::Down);
        assert_eq!(chat.editor.text(), "/time set day");
    }

    #[test]
    fn down_past_newest_restores_unsubmitted_draft() {
        let mut chat = ChatUi::default();
        send(&mut chat, "hello");
        chat.insert_text("partial", 0.0);

        press(&mut chat, petramond_ui::NavKey::Up);
        assert_eq!(chat.editor.text(), "hello");
        press(&mut chat, petramond_ui::NavKey::Down);
        assert_eq!(chat.editor.text(), "partial");
    }

    #[test]
    fn up_with_empty_sent_history_keeps_draft() {
        let mut chat = ChatUi::default();
        chat.insert_text("partial", 0.0);
        press(&mut chat, petramond_ui::NavKey::Up);
        assert_eq!(chat.editor.text(), "partial");
    }

    #[test]
    fn typing_after_recall_detaches_from_history() {
        let mut chat = ChatUi::default();
        send(&mut chat, "hello");
        press(&mut chat, petramond_ui::NavKey::Up);
        chat.insert_text("!", 0.0);
        press(&mut chat, petramond_ui::NavKey::Down);
        assert_eq!(chat.editor.text(), "hello!");
    }
}
