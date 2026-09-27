use crate::doc::{NodeKind, ScrollAxis};
use crate::input::{Drag, FrameState, InputEvent, NavKey, PointerButton, PointerPhase, UiEvent};
use crate::layout::{grid_cell, SlotMetrics, Solved};
use crate::text_edit::TextClipboard;
use crate::theme::Theme;
use crate::tree::{InstKey, InstTree, ROOT};
use crate::widget;

#[derive(Clone, Debug)]
pub(crate) struct SlotRef {
    pub inst: u32,
    pub role: String,
    pub base: u32,
}

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
                    if fs.drag.is_none() {
                        if let Some(i) = self.surface_hit(fs) {
                            if let Some(key) = self.key_of(i) {
                                let (lx, ly) = self.surface_local(fs, i);
                                fs.surface_move = Some((key, lx, ly, None));
                            }
                        }
                    }
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
                InputEvent::Scroll { delta } => self.wheel(fs, delta, events),
                InputEvent::Key { key, shift, ctrl } => {
                    self.key(fs, key, shift, ctrl, clipboard.as_deref_mut(), events);
                }
                InputEvent::Char { ch } => self.chr(fs, ch, events),
                InputEvent::Blur => {
                    fs.active = None;
                    fs.drag = None;
                }
                InputEvent::Modifiers(mods) => fs.mods = mods,
            }
        }
        self.flush_surface_move(fs, events);
        self.surface_hover(fs, events);
    }

    fn surface_hit(&self, fs: &FrameState) -> Option<u32> {
        self.hit(fs)
            .filter(|&i| self.tree.get(i).enabled && self.tree.get(i).node.kind.is_surface())
    }

    fn surface_local(&self, fs: &FrameState, i: u32) -> (f32, f32) {
        let (x, y) = self.cur(fs);
        let rect = self.solved.rects[i as usize];
        (x - rect.x as f32, y - rect.y as f32)
    }

    fn flush_surface_move(&self, fs: &mut FrameState, events: &mut Vec<UiEvent>) {
        if let Some((key, x, y, button)) = fs.surface_move.take() {
            events.push(UiEvent::SurfacePointer {
                id: key.id,
                item: key.item,
                phase: PointerPhase::Move,
                x,
                y,
                button,
                mods: fs.mods,
                clicks: 0,
            });
        }
    }

    fn surface_hover(&self, fs: &mut FrameState, events: &mut Vec<UiEvent>) {
        if matches!(fs.drag, Some(Drag::Surface { .. })) {
            return;
        }
        let now = self.surface_hit(fs).and_then(|i| self.key_of(i));
        if fs.surface_hover != now {
            if let Some(left) = fs.surface_hover.take() {
                let (x, y) = self
                    .tree
                    .find(&left.id, left.item)
                    .map_or((0.0, 0.0), |i| self.surface_local(fs, i));
                events.push(UiEvent::SurfacePointer {
                    id: left.id,
                    item: left.item,
                    phase: PointerPhase::Leave,
                    x,
                    y,
                    button: None,
                    mods: fs.mods,
                    clicks: 0,
                });
            }
            fs.surface_hover = now;
        }
    }

    fn cur(&self, fs: &FrameState) -> (f32, f32) {
        (
            fs.cursor.0 / self.scale as f32,
            fs.cursor.1 / self.scale as f32,
        )
    }

    fn hit(&self, fs: &FrameState) -> Option<u32> {
        let (x, y) = self.cur(fs);
        (0..self.tree.len() as u32).rev().find(|&i| {
            let inst = self.tree.get(i);
            widget::pointer_target(inst) && self.visible_at(i, x, y)
        })
    }

    fn visible_at(&self, i: u32, x: f32, y: f32) -> bool {
        !self.solved.overlay[i as usize]
            && widget::contains_f(self.solved.rects[i as usize], x, y)
            && self.solved.clips[i as usize].is_none_or(|c| widget::contains_f(c, x, y))
    }

    fn row_hit(&self, fs: &FrameState) -> Option<(u32, u32, u32)> {
        let (x, y) = self.cur(fs);
        (0..self.tree.len() as u32).rev().find_map(|i| {
            if !matches!(self.tree.get(i).node.kind, NodeKind::List { .. }) {
                return None;
            }
            let children = &self.tree.get(i).children;
            children
                .iter()
                .position(|&c| self.tree.get(c).enabled && self.visible_at(c, x, y))
                .map(|row| (i, row as u32, children[row]))
        })
    }

    fn paints_over(&self, a: u32, b: u32) -> bool {
        let tier = |i: u32| self.solved.raised[i as usize];
        (tier(a), a) > (tier(b), b)
    }

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

        let row = self.row_hit(fs);
        let widget = self
            .hit(fs)
            .filter(|&i| row.is_none_or(|(_, _, stamp)| !self.paints_over(stamp, i)));
        if let Some(i) = widget {
            let inst = self.tree.get(i);
            let rect = self.solved.rects[i as usize];
            if let Some(key) = &inst.key {
                fs.last_pressed.insert(key.id.clone(), key.clone());
            }
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
                        let font = self.theme.ui_font();
                        let visible = widget::input_visible_chars(font, text_rect.w);
                        let x_rel = x - text_rect.x as f32;
                        if let Some(editor) = fs.editors.get_mut(&key) {
                            let idx = editor.cursor_index_for_x(font, x_rel, visible);
                            let anchor = editor.begin_drag(idx, visible, fs.now);
                            fs.drag = Some(Drag::TextSelect { key, anchor });
                        }
                    }
                }
                NodeKind::Canvas { interactive: true }
                | NodeKind::Viewport { interactive: true }
                    if inst.enabled =>
                {
                    if let Some(key) = self.key_of(i) {
                        self.flush_surface_move(fs, events);
                        let (lx, ly) = self.surface_local(fs, i);
                        let clicks = match &fs.surface_press {
                            Some((k, t, at, n))
                                if *k == key
                                    && fs.now - t < ROW_ACTIVATE_SECS
                                    && (at.0 - fs.cursor.0).abs() <= 4.0
                                    && (at.1 - fs.cursor.1).abs() <= 4.0 =>
                            {
                                n.saturating_add(1)
                            }
                            _ => 1,
                        };
                        fs.surface_press = Some((key.clone(), fs.now, fs.cursor, clicks));
                        events.push(UiEvent::SurfacePointer {
                            id: key.id.clone(),
                            item: key.item,
                            phase: PointerPhase::Down,
                            x: lx,
                            y: ly,
                            button: Some(button),
                            mods: fs.mods,
                            clicks,
                        });
                        fs.drag = Some(Drag::Surface { key, button });
                    }
                    self.blur_editor(fs);
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
                NodeKind::TabBar { tabs } if inst.enabled => {
                    let widths = widget::tab_widths(self.theme, tabs);
                    if let (Some(key), Some(index)) = (
                        self.key_of(i),
                        widget::tab_hit(rect, &widths, self.theme.metrics.tab_gap, x, y),
                    ) {
                        events.push(UiEvent::TabSelect {
                            id: key.id.clone(),
                            index,
                        });
                    }
                    self.blur_editor(fs);
                }
                _ => {}
            }
            return;
        }

        if let Some((list, row, _)) = row {
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
                    let visible = widget::input_visible_chars(self.theme.ui_font(), text_rect.w);
                    let x_rel = x - text_rect.x as f32;
                    if let Some(editor) = fs.editors.get_mut(&key) {
                        let idx = editor.cursor_index_for_x(self.theme.ui_font(), x_rel, visible);
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
            Some(Drag::Surface { key, button }) => {
                if let Some(i) = self.tree.find(&key.id, key.item) {
                    let (lx, ly) = self.surface_local(fs, i);
                    fs.surface_move = Some((key, lx, ly, Some(button)));
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
        if let Some(Drag::Surface {
            key,
            button: drag_button,
        }) = fs.drag.clone()
        {
            if drag_button == button {
                self.flush_surface_move(fs, events);
                fs.drag = None;
                let (x, y) = self
                    .tree
                    .find(&key.id, key.item)
                    .map_or((0.0, 0.0), |i| self.surface_local(fs, i));
                events.push(UiEvent::SurfacePointer {
                    id: key.id,
                    item: key.item,
                    phase: PointerPhase::Up,
                    x,
                    y,
                    button: Some(button),
                    mods: fs.mods,
                    clicks: 0,
                });
            }
            return;
        }
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
                button,
            }),
            _ => {}
        }
    }

    fn wheel(&self, fs: &mut FrameState, delta: i32, events: &mut Vec<UiEvent>) {
        if let Some(i) = self.surface_hit(fs) {
            if let Some(key) = self.key_of(i) {
                let (x, y) = self.surface_local(fs, i);
                events.push(UiEvent::SurfaceScroll {
                    id: key.id,
                    item: key.item,
                    x,
                    y,
                    delta,
                    mods: fs.mods,
                });
            }
            return;
        }
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
        ctrl: bool,
        clipboard: Option<&mut C>,
        events: &mut Vec<UiEvent>,
    ) {
        if key == NavKey::Tab && self.focus_next_input(fs, shift) {
            return;
        }

        if let Some(focus) = fs.focus.clone() {
            if let Some(i) = self.tree.find(&focus.id, focus.item) {
                let rect = self.solved.rects[i as usize];
                let pad = self.theme.metrics.button_pad;
                let text_w = widget::input_text_rect(rect, pad).w;
                let visible = widget::input_visible_chars(self.theme.ui_font(), text_w);
                let now = fs.now;
                if let Some(editor) = fs.editors.get_mut(&focus) {
                    let before = editor.text().to_owned();
                    let mut consumed = true;
                    match (key, ctrl) {
                        (NavKey::Left, true) => {
                            editor.move_word_left(shift, visible, now);
                        }
                        (NavKey::Right, true) => {
                            editor.move_word_right(shift, visible, now);
                        }
                        (NavKey::Left, false) => {
                            editor.move_left(shift, visible, now);
                        }
                        (NavKey::Right, false) => {
                            editor.move_right(shift, visible, now);
                        }
                        (NavKey::Home, _) => editor.move_home(shift, visible, now),
                        (NavKey::End, _) => editor.move_end(shift, visible, now),
                        (NavKey::Backspace, true) => {
                            editor.backspace_word(visible, now);
                        }
                        (NavKey::Backspace, false) => {
                            editor.backspace(visible, now);
                        }
                        (NavKey::Delete, true) => {
                            editor.delete_word_forward(visible, now);
                        }
                        (NavKey::Delete, false) => {
                            editor.delete_forward(visible, now);
                        }
                        (NavKey::SelectAll, _) => {
                            editor.select_all(visible, now);
                        }
                        (NavKey::Copy, _) => {
                            if let Some(mut cb) = clipboard {
                                editor.copy_selection(&mut cb);
                            }
                        }
                        (NavKey::Cut, _) => {
                            if let Some(mut cb) = clipboard {
                                editor.cut_selection(&mut cb, visible, now);
                            }
                        }
                        (NavKey::Paste, _) => {
                            if let Some(mut cb) = clipboard {
                                editor.paste(&mut cb, visible, now);
                            }
                        }
                        (NavKey::Enter, _) => {
                            events.push(UiEvent::Submit {
                                id: focus.id.clone(),
                                text: editor.text().to_owned(),
                            });
                        }
                        (NavKey::Escape, _) => {
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
        events.push(UiEvent::Key { key, shift, ctrl });
    }

    fn focus_next_input(&self, fs: &mut FrameState, back: bool) -> bool {
        let inputs: Vec<(InstKey, String, usize)> = self
            .tree
            .insts
            .iter()
            .filter_map(|inst| {
                if !inst.enabled {
                    return None;
                }
                let NodeKind::TextInput { max_chars, .. } = &inst.node.kind else {
                    return None;
                };
                let key = inst.key.clone()?;
                let bound = inst.text.clone().unwrap_or_default();
                Some((key, bound, *max_chars))
            })
            .collect();
        if inputs.is_empty() {
            return false;
        }
        let cur = fs.focus.as_ref();
        let idx = cur
            .and_then(|f| inputs.iter().position(|(k, ..)| k == f))
            .unwrap_or(if back { 0 } else { inputs.len() - 1 });
        let next = if back {
            if idx == 0 {
                inputs.len() - 1
            } else {
                idx - 1
            }
        } else {
            (idx + 1) % inputs.len()
        };
        let (key, bound, max_chars) = &inputs[next];
        if fs.focus.as_ref() != Some(key) {
            self.blur_editor(fs);
            fs.focus_text_input(key.clone(), bound, *max_chars);
        }
        true
    }

    fn chr(&self, fs: &mut FrameState, ch: char, events: &mut Vec<UiEvent>) {
        let Some(focus) = fs.focus.clone() else {
            if ch.is_ascii_alphanumeric() {
                events.push(UiEvent::Key {
                    key: NavKey::Char(ch.to_ascii_lowercase()),
                    shift: ch.is_ascii_uppercase(),
                    ctrl: false,
                });
            }
            return;
        };
        let Some(i) = self.tree.find(&focus.id, focus.item) else {
            return;
        };
        let rect = self.solved.rects[i as usize];
        let pad = self.theme.metrics.button_pad;
        let text_w = widget::input_text_rect(rect, pad).w;
        let visible = widget::input_visible_chars(self.theme.ui_font(), text_w);
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
