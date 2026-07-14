//! The interaction pass: host input events against the frame's solved
//! geometry, mutating [`FrameState`] and emitting [`UiEvent`]s.
//!
//! Semantics preserved from the legacy GUI:
//! - Ordinary slot and list-row presses fire on pointer **down**. A host-armed
//!   cursor-stack slot gesture captures distinct cells through release.
//! - Buttons, checkboxes, and toggles fire on press-in-**release**-in.
//! - A press that hits nothing outside the panel emits `ClickOutside`
//!   (cursor-stack throws).
//! - Keys no widget consumes surface as `UiEvent::Key` for per-screen
//!   controllers (list nav, ESC-back, Delete jumps).

use crate::doc::{NodeKind, ScrollAxis};
use crate::input::{Drag, FrameState, InputEvent, NavKey, PointerButton, PointerPhase, UiEvent};
use crate::layout::{grid_cell, SlotMetrics, Solved};
use crate::text_edit::TextClipboard;
use crate::theme::Theme;
use crate::tree::{InstKey, InstTree, ROOT};
use crate::widget;

/// One slot-bearing instance and its role's starting in-role index.
#[derive(Clone, Debug)]
pub(crate) struct SlotRef {
    pub inst: u32,
    pub role: String,
    pub base: u32,
}

/// Collect slot instances in arena (document) order with their per-role base
/// indices — the same accumulation as [`crate::doc::Document::role_slots`].
pub(crate) fn collect_slots(tree: &InstTree<'_>) -> Vec<SlotRef> {
    let mut counts: Vec<(String, u32)> = Vec::new();
    let mut out = Vec::new();
    for (i, inst) in tree.insts.iter().enumerate() {
        let (role, n) = match &inst.node.kind {
            NodeKind::Slot { role, .. } => (role, 1),
            NodeKind::SlotGrid {
                role, cols, rows, ..
            } => (role, cols * rows),
            _ => continue,
        };
        let base = match counts.iter_mut().find(|(r, _)| r == role) {
            Some((_, c)) => {
                let b = *c;
                *c += n;
                b
            }
            None => {
                counts.push((role.clone(), n));
                0
            }
        };
        out.push(SlotRef {
            inst: i as u32,
            role: role.clone(),
            base,
        });
    }
    out
}

/// The double-click window for list-row activation, seconds.
const ROW_ACTIVATE_SECS: f64 = 0.25;

pub(crate) struct Interact<'a> {
    pub tree: &'a InstTree<'a>,
    pub solved: &'a Solved,
    pub theme: &'a Theme,
    pub scale: i32,
    pub slots: &'a [SlotRef],
    pub metrics: SlotMetrics,
}

impl Interact<'_> {
    /// Process every event in order, mutating `fs` and appending to `events`.
    pub fn run<C: TextClipboard + ?Sized>(
        &self,
        fs: &mut FrameState,
        input: &[InputEvent],
        mut clipboard: Option<&mut C>,
        events: &mut Vec<UiEvent>,
    ) {
        for ev in input {
            match *ev {
                InputEvent::PointerMove { x, y } => {
                    fs.cursor = (x, y);
                    self.pointer_drag(fs, events);
                }
                InputEvent::PointerDown {
                    x,
                    y,
                    button,
                    shift,
                    slot_drag,
                } => {
                    fs.cursor = (x, y);
                    self.pointer_down(fs, button, shift, slot_drag, events);
                }
                InputEvent::PointerUp { x, y, button } => {
                    fs.cursor = (x, y);
                    self.pointer_up(fs, button, events);
                }
                InputEvent::Scroll { delta } => self.wheel(fs, delta),
                InputEvent::Key { key, shift } => {
                    self.key(fs, key, shift, clipboard.as_deref_mut(), events);
                }
                InputEvent::Char { ch } => self.chr(fs, ch, events),
                InputEvent::Blur => {
                    fs.active = None;
                    fs.drag = None;
                }
            }
        }
    }

    /// Cursor in logical px.
    fn cur(&self, fs: &FrameState) -> (f32, f32) {
        (
            fs.cursor.0 / self.scale as f32,
            fs.cursor.1 / self.scale as f32,
        )
    }

    /// The topmost pointer-target instance under the cursor (arena order is
    /// paint order, so scan backwards), respecting clips.
    fn hit(&self, fs: &FrameState) -> Option<u32> {
        let (x, y) = self.cur(fs);
        (0..self.tree.len() as u32).rev().find(|&i| {
            let inst = self.tree.get(i);
            widget::pointer_target(inst) && self.visible_at(i, x, y)
        })
    }

    fn visible_at(&self, i: u32, x: f32, y: f32) -> bool {
        widget::contains_f(self.solved.rects[i as usize], x, y)
            && self.solved.clips[i as usize].is_none_or(|c| widget::contains_f(c, x, y))
    }

    /// The deepest list-template stamp (list direct child) under the cursor,
    /// as `(list inst, row index)`.
    fn row_hit(&self, fs: &FrameState) -> Option<(u32, u32)> {
        let (x, y) = self.cur(fs);
        (0..self.tree.len() as u32).rev().find_map(|i| {
            if !matches!(self.tree.get(i).node.kind, NodeKind::List) {
                return None;
            }
            self.tree
                .get(i)
                .children
                .iter()
                .position(|&c| self.tree.get(c).enabled && self.visible_at(c, x, y))
                .map(|row| (i, row as u32))
        })
    }

    /// The deepest scroll node under the cursor.
    fn scroll_hit(&self, fs: &FrameState) -> Option<u32> {
        let (x, y) = self.cur(fs);
        (0..self.tree.len() as u32).rev().find(|&i| {
            self.tree.get(i).enabled
                && matches!(self.tree.get(i).node.kind, NodeKind::Scroll { .. })
                && self.visible_at(i, x, y)
        })
    }

    fn key_of(&self, i: u32) -> Option<InstKey> {
        self.tree.get(i).key.clone()
    }

    /// The enabled slot cell under the cursor, resolved to its stable
    /// document role + in-role index.
    fn slot_hit(&self, fs: &FrameState) -> Option<(String, u32)> {
        let i = self.hit(fs)?;
        let inst = self.tree.get(i);
        if !inst.enabled {
            return None;
        }
        let slot = self.slots.iter().find(|slot| slot.inst == i)?;
        let (x, y) = self.cur(fs);
        let rect = self.solved.rects[i as usize];
        let cell = match inst.node.kind {
            NodeKind::SlotGrid { cols, rows, .. } => (0..cols * rows).find(|&cell| {
                widget::contains_f(grid_cell(rect, cols, cell, self.metrics), x, y)
            })?,
            NodeKind::Slot { .. } => 0,
            _ => return None,
        };
        Some((slot.role.clone(), slot.base + cell))
    }

    fn pointer_down(
        &self,
        fs: &mut FrameState,
        button: PointerButton,
        shift: bool,
        slot_drag: bool,
        events: &mut Vec<UiEvent>,
    ) {
        let (x, y) = self.cur(fs);

        // Scrollbar thumbs sit above content: check them first.
        for i in (0..self.tree.len() as u32).rev() {
            let inst = self.tree.get(i);
            let NodeKind::Scroll { axis } = inst.node.kind else {
                continue;
            };
            if !inst.enabled || axis != ScrollAxis::Vertical {
                continue;
            }
            let rect = self.solved.rects[i as usize];
            let view = widget::scroll_view_rect(self.theme, inst.node, rect);
            let content = self.solved.scroll_content[i as usize].unwrap_or((0, 0));
            let Some(key) = self.key_of(i) else { continue };
            let offset = fs.scroll_offset(&key);
            let Some((track, thumb)) = widget::scrollbar(
                view,
                rect.h,
                content.1,
                offset,
                self.theme.metrics.scrollbar_w,
            ) else {
                continue;
            };
            if widget::contains_f(thumb, x, y) {
                fs.drag = Some(Drag::ScrollThumb {
                    key,
                    grab: y - thumb.y as f32,
                });
                return;
            }
            if widget::contains_f(track, x, y) {
                // Track click: jump the thumb centre to the pointer.
                let new_off = widget::scroll_offset_for_thumb_y(
                    view,
                    rect.h,
                    content.1,
                    y - thumb.h as f32 / 2.0,
                );
                fs.set_scroll(key, widget::clamp_scroll(new_off, rect.h, content.1));
                return;
            }
        }

        if let Some(i) = self.hit(fs) {
            let inst = self.tree.get(i);
            let rect = self.solved.rects[i as usize];
            match &inst.node.kind {
                NodeKind::Slot { .. } | NodeKind::SlotGrid { .. } => {
                    if let Some((role, index)) = self.slot_hit(fs) {
                        if slot_drag {
                            fs.drag = Some(Drag::Slots {
                                button,
                                shift,
                                slots: vec![(role, index)],
                            });
                        } else {
                            events.push(UiEvent::SlotClick {
                                role,
                                index,
                                button,
                                shift,
                            });
                        }
                    }
                    self.blur_editor(fs);
                }
                NodeKind::Button { .. } | NodeKind::Checkbox | NodeKind::Toggle { .. } => {
                    if inst.enabled {
                        if let Some(key) = self.key_of(i) {
                            fs.active = Some((key, button));
                        }
                    }
                    self.blur_editor(fs);
                }
                NodeKind::Slider { min, max, step } => {
                    if inst.enabled {
                        if let Some(key) = self.key_of(i) {
                            let value = widget::slider_value_at(rect, x, *min, *max, *step);
                            events.push(UiEvent::SliderChange {
                                id: key.id.clone(),
                                item: key.item,
                                value,
                                committed: false,
                            });
                            fs.drag = Some(Drag::Slider { key });
                        }
                    }
                    self.blur_editor(fs);
                }
                NodeKind::TextInput { max_chars, .. } if inst.enabled => {
                    if let Some(key) = self.key_of(i) {
                        let bound = inst.text.clone().unwrap_or_default();
                        fs.focus_text_input(key.clone(), &bound, *max_chars);
                        let pad = self.theme.metrics.button_pad;
                        let text_rect = widget::input_text_rect(rect, pad);
                        let visible = widget::input_visible_chars(text_rect.w);
                        let x_rel = (x - text_rect.x as f32) * self.scale as f32;
                        let advance = (crate::text::ADVANCE * self.scale) as f32;
                        if let Some(editor) = fs.editors.get_mut(&key) {
                            let idx = editor.cursor_index_for_x(x_rel, advance, visible);
                            let anchor = editor.begin_drag(idx, visible, fs.now);
                            fs.drag = Some(Drag::TextSelect { key, anchor });
                        }
                    }
                }
                NodeKind::Image {
                    interactive: true, ..
                } if inst.enabled => {
                    if let Some(key) = self.key_of(i) {
                        events.push(UiEvent::ImagePointer {
                            id: key.id.clone(),
                            phase: PointerPhase::Down,
                            x: x - rect.x as f32,
                            y: y - rect.y as f32,
                            button,
                        });
                        fs.drag = Some(Drag::Image { key, button });
                    }
                    self.blur_editor(fs);
                }
                _ => {}
            }
            return;
        }

        if let Some((list, row)) = self.row_hit(fs) {
            self.blur_editor(fs);
            if let Some(key) = self.key_of(list) {
                events.push(UiEvent::ListSelect {
                    id: key.id.clone(),
                    index: row,
                });
                let doubled = fs.last_row_click.as_ref().is_some_and(|(k, r, t)| {
                    *k == key && *r == row && fs.now - t < ROW_ACTIVATE_SECS
                });
                if doubled {
                    events.push(UiEvent::ListActivate {
                        id: key.id.clone(),
                        index: row,
                    });
                    fs.last_row_click = None;
                } else {
                    fs.last_row_click = Some((key, row, fs.now));
                }
            }
            return;
        }

        self.blur_editor(fs);
        let (x, y) = self.cur(fs);
        if !widget::contains_f(self.solved.rects[ROOT as usize], x, y) {
            events.push(UiEvent::ClickOutside { button });
        }
    }

    fn pointer_drag(&self, fs: &mut FrameState, events: &mut Vec<UiEvent>) {
        let (x, y) = self.cur(fs);
        let slot_hit = self.slot_hit(fs);
        match fs.drag.clone() {
            Some(Drag::Slider { key }) => {
                if let Some(i) = self.tree.find(&key.id, key.item) {
                    if let NodeKind::Slider { min, max, step } = self.tree.get(i).node.kind {
                        let rect = self.solved.rects[i as usize];
                        let value = widget::slider_value_at(rect, x, min, max, step);
                        events.push(UiEvent::SliderChange {
                            id: key.id.clone(),
                            item: key.item,
                            value,
                            committed: false,
                        });
                    }
                }
            }
            Some(Drag::ScrollThumb { key, grab }) => {
                if let Some(i) = self.tree.find(&key.id, key.item) {
                    let rect = self.solved.rects[i as usize];
                    let view = widget::scroll_view_rect(self.theme, self.tree.get(i).node, rect);
                    let content = self.solved.scroll_content[i as usize].unwrap_or((0, 0));
                    let off = widget::scroll_offset_for_thumb_y(view, rect.h, content.1, y - grab);
                    fs.set_scroll(key, widget::clamp_scroll(off, rect.h, content.1));
                }
            }
            Some(Drag::TextSelect { key, anchor }) => {
                if let Some(i) = self.tree.find(&key.id, key.item) {
                    let rect = self.solved.rects[i as usize];
                    let pad = self.theme.metrics.button_pad;
                    let text_rect = widget::input_text_rect(rect, pad);
                    let visible = widget::input_visible_chars(text_rect.w);
                    let x_rel = (x - text_rect.x as f32) * self.scale as f32;
                    let advance = (crate::text::ADVANCE * self.scale) as f32;
                    if let Some(editor) = fs.editors.get_mut(&key) {
                        let idx = editor.cursor_index_for_x(x_rel, advance, visible);
                        editor.drag_to(anchor, idx, visible, fs.now);
                    }
                }
            }
            Some(Drag::Image { key, button }) => {
                if let Some(i) = self.tree.find(&key.id, key.item) {
                    let rect = self.solved.rects[i as usize];
                    events.push(UiEvent::ImagePointer {
                        id: key.id,
                        phase: PointerPhase::Move,
                        x: x - rect.x as f32,
                        y: y - rect.y as f32,
                        button,
                    });
                }
            }
            Some(Drag::Slots {
                button,
                shift,
                mut slots,
            }) => {
                if let Some(slot) = slot_hit {
                    if !slots.contains(&slot) {
                        slots.push(slot);
                    }
                }
                fs.drag = Some(Drag::Slots {
                    button,
                    shift,
                    slots,
                });
            }
            None => {}
        }
    }

    fn pointer_up(&self, fs: &mut FrameState, button: PointerButton, events: &mut Vec<UiEvent>) {
        if let Some(Drag::Slots {
            button: drag_button,
            shift,
            slots,
        }) = fs.drag.clone()
        {
            if drag_button != button {
                return;
            }
            fs.drag = None;
            if slots.len() == 1 {
                let (role, index) = slots.into_iter().next().expect("one slot");
                events.push(UiEvent::SlotClick {
                    role,
                    index,
                    button,
                    shift,
                });
            } else if !slots.is_empty() {
                events.push(UiEvent::SlotDrag { slots, button });
            }
            return;
        }
        if let Some(Drag::Image {
            key,
            button: drag_button,
        }) = fs.drag.clone()
        {
            if drag_button == button {
                if let Some(i) = self.tree.find(&key.id, key.item) {
                    let rect = self.solved.rects[i as usize];
                    let (x, y) = self.cur(fs);
                    events.push(UiEvent::ImagePointer {
                        id: key.id,
                        phase: PointerPhase::Up,
                        x: x - rect.x as f32,
                        y: y - rect.y as f32,
                        button,
                    });
                }
            }
        }
        if let Some(Drag::Slider { key }) = fs.drag.clone() {
            let (x, _) = self.cur(fs);
            if let Some(i) = self.tree.find(&key.id, key.item) {
                if let NodeKind::Slider { min, max, step } = self.tree.get(i).node.kind {
                    let rect = self.solved.rects[i as usize];
                    let value = widget::slider_value_at(rect, x, min, max, step);
                    events.push(UiEvent::SliderChange {
                        id: key.id.clone(),
                        item: key.item,
                        value,
                        committed: true,
                    });
                }
            }
        }
        fs.drag = None;

        let Some((key, press_button)) = fs.active.take() else {
            return;
        };
        if press_button != button {
            fs.active = Some((key, press_button));
            return;
        }
        // Release-in fires; release-out cancels.
        let Some(i) = self.tree.find(&key.id, key.item) else {
            return;
        };
        let (x, y) = self.cur(fs);
        if !self.visible_at(i, x, y) {
            return;
        }
        let inst = self.tree.get(i);
        if !inst.enabled {
            return;
        }
        match inst.node.kind {
            NodeKind::Button { .. } => {
                // Keep the pressed face through the click frame: the host
                // applies the event (e.g. a list selection) before the NEXT
                // frame, so without this the row flashes unpressed once.
                fs.clicked = Some(key.clone());
                events.push(UiEvent::Click {
                    id: key.id,
                    item: key.item,
                    button,
                });
            }
            NodeKind::Checkbox | NodeKind::Toggle { .. } => events.push(UiEvent::Toggle {
                id: key.id,
                item: key.item,
                on: !inst.value_bool.unwrap_or(false),
            }),
            _ => {}
        }
    }

    fn wheel(&self, fs: &mut FrameState, delta: i32) {
        let Some(i) = self.scroll_hit(fs) else {
            return;
        };
        let inst = self.tree.get(i);
        let NodeKind::Scroll { axis } = inst.node.kind else {
            return;
        };
        let Some(key) = self.key_of(i) else { return };
        let rect = self.solved.rects[i as usize];
        let content = self.solved.scroll_content[i as usize].unwrap_or((0, 0));
        let (viewport, content_len) = widget::scroll_lengths(axis, rect, content);
        let off = widget::clamp_scroll(fs.scroll_offset(&key) + delta, viewport, content_len);
        fs.set_scroll(key, off);
    }

    fn key<C: TextClipboard + ?Sized>(
        &self,
        fs: &mut FrameState,
        key: NavKey,
        shift: bool,
        clipboard: Option<&mut C>,
        events: &mut Vec<UiEvent>,
    ) {
        // A focused editor consumes editing keys.
        if let Some(focus) = fs.focus.clone() {
            if let Some(i) = self.tree.find(&focus.id, focus.item) {
                let rect = self.solved.rects[i as usize];
                let pad = self.theme.metrics.button_pad;
                let visible = widget::input_visible_chars(widget::input_text_rect(rect, pad).w);
                let now = fs.now;
                if let Some(editor) = fs.editors.get_mut(&focus) {
                    let before = editor.text().to_owned();
                    let mut consumed = true;
                    match key {
                        NavKey::Left => {
                            editor.move_left(shift, visible, now);
                        }
                        NavKey::Right => {
                            editor.move_right(shift, visible, now);
                        }
                        NavKey::Home => editor.move_home(shift, visible, now),
                        NavKey::End => editor.move_end(shift, visible, now),
                        NavKey::Backspace => {
                            editor.backspace(visible, now);
                        }
                        NavKey::Delete => {
                            editor.delete_forward(visible, now);
                        }
                        NavKey::SelectAll => {
                            editor.select_all(visible, now);
                        }
                        NavKey::Copy => {
                            if let Some(mut cb) = clipboard {
                                editor.copy_selection(&mut cb);
                            }
                        }
                        NavKey::Cut => {
                            if let Some(mut cb) = clipboard {
                                editor.cut_selection(&mut cb, visible, now);
                            }
                        }
                        NavKey::Paste => {
                            if let Some(mut cb) = clipboard {
                                editor.paste(&mut cb, visible, now);
                            }
                        }
                        NavKey::Enter => {
                            events.push(UiEvent::Submit {
                                id: focus.id.clone(),
                                text: editor.text().to_owned(),
                            });
                        }
                        NavKey::Escape => {
                            editor.blur();
                            fs.focus = None;
                        }
                        _ => consumed = false,
                    }
                    let after = fs
                        .editors
                        .get(&focus)
                        .map(|e| e.text().to_owned())
                        .unwrap_or_default();
                    if after != before {
                        events.push(UiEvent::TextChanged {
                            id: focus.id.clone(),
                            text: after,
                        });
                    }
                    if consumed {
                        return;
                    }
                }
            }
        }
        events.push(UiEvent::Key { key, shift });
    }

    fn chr(&self, fs: &mut FrameState, ch: char, events: &mut Vec<UiEvent>) {
        let Some(focus) = fs.focus.clone() else {
            return;
        };
        let Some(i) = self.tree.find(&focus.id, focus.item) else {
            return;
        };
        let rect = self.solved.rects[i as usize];
        let pad = self.theme.metrics.button_pad;
        let visible = widget::input_visible_chars(widget::input_text_rect(rect, pad).w);
        let now = fs.now;
        if let Some(editor) = fs.editors.get_mut(&focus) {
            if editor.insert_text(&ch.to_string(), visible, now) {
                events.push(UiEvent::TextChanged {
                    id: focus.id.clone(),
                    text: editor.text().to_owned(),
                });
            }
        }
    }

    fn blur_editor(&self, fs: &mut FrameState) {
        if let Some(focus) = fs.focus.take() {
            if let Some(editor) = fs.editors.get_mut(&focus) {
                editor.blur();
            }
        }
    }
}
