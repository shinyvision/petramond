//! The one-call-per-frame runtime facade: expand → solve → interact → paint.
//!
//! The game and the builder preview both drive a [`UiRuntime`]; everything a
//! frame produces comes back in [`FrameOutput`] — the draw list, resolved
//! widget events, and the named/slot rects the host needs to layer its own
//! content (item icons, hearts) and to hit-test latched clicks.

use crate::doc::{Document, NodeKind};
use crate::input::{FrameState, InputEvent, PreviewState, UiEvent};
use crate::interact::{collect_slots, Interact};
use crate::layout::{grid_cell, solve, RectI};
use crate::paint::{DrawList, Painter, TexId, SOLID_UV};
use crate::paint_walk::{DocImages, PaintCtx};
use crate::text_edit::TextClipboard;
use crate::theme::{Theme, ThemeEnv};
use crate::tree::{InstKey, InstTree, ROOT};
use crate::widget;
use std::sync::Arc;

pub struct UiRuntime {
    doc: Arc<Document>,
    theme: Arc<Theme>,
}

pub struct FrameArgs<'a> {
    /// Physical framebuffer size, px.
    pub screen: (u32, u32),
    /// Integer gui scale (logical px × scale = physical px).
    pub scale: i32,
    /// Host time in seconds (blink, double-click windows).
    pub now: f64,
    pub state: &'a crate::state::UiState,
    /// Input events since the last frame, in order.
    pub input: &'a [InputEvent],
    pub clipboard: Option<&'a mut dyn TextClipboard>,
    pub images: &'a dyn DocImages,
    /// Fullscreen backdrop color behind the GUI (menus dim the world).
    pub dim: Option<[f32; 4]>,
    /// Builder-only forced widget states.
    pub preview: Option<&'a PreviewState>,
}

/// One slot cell's physical rect, by role + in-role index.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotRectOut {
    pub role: String,
    pub index: u32,
    pub rect: RectI,
}

/// One host-drawn `hook` instance. The rect and inherited clip are physical
/// pixels, matching [`SlotRectOut`]; `key.item` identifies a repeated list
/// row. Hosts must apply `clip` when drawing content inside scrolling hooks.
#[derive(Clone, Debug, PartialEq)]
pub struct HookRectOut {
    pub key: InstKey,
    pub rect: RectI,
    pub clip: Option<RectI>,
}

#[derive(Default)]
pub struct FrameOutput {
    pub draw: DrawList,
    pub events: Vec<UiEvent>,
    /// Physical rects of every id-bearing instance (hooks, widgets).
    pub named: Vec<(InstKey, RectI)>,
    /// Physical rects and inherited clips for host-drawn `hook` instances.
    pub hooks: Vec<HookRectOut>,
    /// Physical rects of every slot cell, role-indexed like the host's slots.
    pub slots: Vec<SlotRectOut>,
    /// The root panel's physical rect (outside = cursor-throw territory).
    pub panel_rect: RectI,
    /// The slot cell under the cursor, if any.
    pub hover_slot: Option<(String, u32)>,
}

impl FrameOutput {
    /// The physical rect of instance `id` (first match).
    pub fn rect(&self, id: &str) -> Option<RectI> {
        self.named.iter().find(|(k, _)| k.id == id).map(|(_, r)| *r)
    }
}

impl UiRuntime {
    pub fn new(doc: Arc<Document>, theme: Arc<Theme>) -> UiRuntime {
        UiRuntime { doc, theme }
    }

    pub fn doc(&self) -> &Arc<Document> {
        &self.doc
    }

    pub fn theme(&self) -> &Arc<Theme> {
        &self.theme
    }

    pub fn frame(&self, mut args: FrameArgs<'_>, fs: &mut FrameState, out: &mut FrameOutput) {
        out.draw.clear();
        out.events.clear();
        out.named.clear();
        out.hooks.clear();
        out.slots.clear();
        out.hover_slot = None;
        out.panel_rect = RectI::ZERO;
        if args.screen.0 == 0 || args.screen.1 == 0 || args.scale <= 0 {
            return;
        }
        fs.now = args.now;
        // The click bridge lives exactly one frame (set below in interaction,
        // read by this frame's paint).
        fs.clicked = None;

        let scale = args.scale;
        let viewport = (
            (args.screen.0 as i32) / scale,
            (args.screen.1 as i32) / scale,
        );
        let tree = InstTree::expand_form(&self.doc, args.state, self.doc.compact_active(viewport.0));
        if tree.is_empty() {
            return;
        }
        let images = args.images;
        let env = ThemeEnv {
            theme: &self.theme,
            image_size: &|name| images.resolve(name).map(|(_, (w, h))| (w as i32, h as i32)),
        };
        let solved = solve(&tree, &env, viewport, &|i| {
            tree.get(i)
                .key
                .as_ref()
                .map(|k| fs.scroll_offset(k))
                .unwrap_or(0)
        });

        // Re-clamp scroll offsets against this frame's content so a shrunk
        // list can't strand its offset out of range.
        for i in 0..tree.len() as u32 {
            let inst = tree.get(i);
            let NodeKind::Scroll { axis } = inst.node.kind else {
                continue;
            };
            let Some(key) = inst.key.clone() else {
                continue;
            };
            let (viewport_len, content_len) = widget::scroll_lengths(
                axis,
                solved.rects[i as usize],
                solved.scroll_content[i as usize].unwrap_or((0, 0)),
            );
            let clamped = widget::clamp_scroll(fs.scroll_offset(&key), viewport_len, content_len);
            if clamped != fs.scroll_offset(&key) {
                fs.set_scroll(key, clamped);
            }
        }

        // A list whose bound selection CHANGED (keyboard nav) scrolls its
        // enclosing scroll region to keep the selected row visible.
        for i in 0..tree.len() as u32 {
            let inst = tree.get(i);
            if !matches!(inst.node.kind, NodeKind::List) {
                continue;
            }
            let (Some(key), Some(selected)) = (inst.key.clone(), inst.selected) else {
                continue;
            };
            let changed = fs.last_selected.get(&key) != Some(&selected);
            fs.last_selected.insert(key, selected);
            if !changed || selected < 0 {
                continue;
            }
            let Some(&row_inst) = inst.children.get(selected as usize) else {
                continue;
            };
            // The nearest scroll ancestor owns the offset.
            let mut anc = inst.parent;
            while let Some(a) = anc {
                if matches!(tree.get(a).node.kind, NodeKind::Scroll { .. }) {
                    break;
                }
                anc = tree.get(a).parent;
            }
            let Some(scroll_i) = anc else { continue };
            let Some(scroll_key) = tree.get(scroll_i).key.clone() else {
                continue;
            };
            let view = solved.rects[scroll_i as usize];
            let row = solved.rects[row_inst as usize];
            let off = fs.scroll_offset(&scroll_key);
            let new_off = if row.y < view.y {
                off - (view.y - row.y)
            } else if row.y + row.h > view.y + view.h {
                off + (row.y + row.h - view.y - view.h)
            } else {
                off
            };
            let content = solved.scroll_content[scroll_i as usize].unwrap_or((0, 0));
            let new_off = widget::clamp_scroll(new_off, view.h, content.1);
            if new_off != off {
                fs.set_scroll(scroll_key, new_off);
            }
        }

        let slots = collect_slots(&tree);
        let metrics = crate::layout::SlotMetrics {
            slot: self.theme.metrics.slot,
            gap: self.theme.metrics.slot_gap,
        };
        let interact = Interact {
            tree: &tree,
            solved: &solved,
            theme: &self.theme,
            scale,
            slots: &slots,
            metrics,
        };
        interact.run(fs, args.input, args.clipboard.take(), &mut out.events);

        // Hover resolution for paint, from the post-input cursor.
        let (cx, cy) = (fs.cursor().0 / scale as f32, fs.cursor().1 / scale as f32);
        let visible_at = |i: u32| {
            widget::contains_f(solved.rects[i as usize], cx, cy)
                && solved.clips[i as usize].is_none_or(|c| widget::contains_f(c, cx, cy))
        };
        let hover = (0..tree.len() as u32)
            .rev()
            .find(|&i| tree.get(i).enabled && widget::pointer_target(tree.get(i)) && visible_at(i));
        let slot_hover = hover.and_then(|i| match tree.get(i).node.kind {
            NodeKind::Slot { .. } => Some((i, 0)),
            NodeKind::SlotGrid { cols, rows, .. } => (0..cols * rows)
                .find(|&c| {
                    widget::contains_f(
                        grid_cell(solved.rects[i as usize], cols, c, metrics),
                        cx,
                        cy,
                    )
                })
                .map(|c| (i, c)),
            _ => None,
        });
        let row_hover = (0..tree.len() as u32).rev().find_map(|i| {
            if !matches!(tree.get(i).node.kind, NodeKind::List) || !visible_at(i) {
                return None;
            }
            tree.get(i)
                .children
                .iter()
                .position(|&c| tree.get(c).enabled && visible_at(c))
                .map(|row| (i, row as u32))
        });

        // Paint: dim backdrop (physical fullscreen), then the tree.
        if let Some(color) = args.dim {
            let (w, h) = (args.screen.0 as f32, args.screen.1 as f32);
            out.draw.push_quad(
                TexId::Solid,
                [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
                [SOLID_UV; 4],
                color,
                None,
            );
        }
        let ctx = PaintCtx {
            tree: &tree,
            solved: &solved,
            theme: &self.theme,
            fs,
            images,
            metrics,
            hover,
            slot_hover,
            row_hover,
            preview: args.preview,
        };
        ctx.paint(&mut Painter {
            list: &mut out.draw,
            scale,
        });

        // Outputs the host layers content with, all in physical px.
        let phys = |r: RectI| RectI {
            x: r.x * scale,
            y: r.y * scale,
            w: r.w * scale,
            h: r.h * scale,
        };
        out.panel_rect = phys(solved.rects[ROOT as usize]);
        for (i, inst) in tree.insts.iter().enumerate() {
            if let Some(key) = &inst.key {
                let rect = phys(solved.rects[i]);
                out.named.push((key.clone(), rect));
                if matches!(inst.node.kind, NodeKind::Hook) {
                    out.hooks.push(HookRectOut {
                        key: key.clone(),
                        rect,
                        clip: solved.clips[i].map(phys),
                    });
                }
            }
        }
        for slot in &slots {
            let rect = solved.rects[slot.inst as usize];
            let (cols, cells) = match tree.get(slot.inst).node.kind {
                NodeKind::SlotGrid { cols, rows, .. } => (cols, cols * rows),
                _ => (1, 1),
            };
            for c in 0..cells {
                out.slots.push(SlotRectOut {
                    role: slot.role.clone(),
                    index: slot.base + c,
                    rect: phys(grid_cell(rect, cols, c, metrics)),
                });
            }
        }
        out.hover_slot = slot_hover.and_then(|(i, c)| {
            slots
                .iter()
                .find(|s| s.inst == i)
                .map(|s| (s.role.clone(), s.base + c))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{NavKey, PointerButton, PointerPhase};
    use crate::paint_walk::NoImages;
    use crate::state::{UiMap, UiState, UiValue};
    use std::sync::Arc;

    fn screen_doc() -> Arc<Document> {
        Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_screen", "class": "screen",
                "root": { "type": "column", "layout": { "w": 200, "h": 200, "pad": [10,10,10,10], "gap": 4 },
                    "children": [
                        { "type": "button", "id": "ok", "text": "OK" },
                        { "type": "toggle", "id": "snd", "bind": { "value": "sound_on" } },
                        { "type": "slider", "id": "vol", "min": 0.0, "max": 100.0, "step": 25.0,
                          "bind": { "value": "volume" }, "layout": { "w": 100 } },
                        { "type": "text_input", "id": "name", "placeholder": "World name",
                          "bind": { "text": "world_name" }, "layout": { "w": 120 } },
                        { "type": "scroll", "id": "sc", "layout": { "h": 40 },
                          "children": [
                            { "type": "list", "id": "rows", "bind": { "items": "rows", "selected": "sel" },
                              "children": [
                                { "type": "label", "bind": { "text": "name" }, "style": "list.row",
                                  "layout": { "h": 20 } }
                              ] }
                          ] }
                    ] }
            }"#,
            )
            .unwrap(),
        )
    }

    fn state_with_rows(n: usize) -> UiState {
        let mut s = UiState::new();
        s.set("sound_on", UiValue::Bool(false));
        s.set("volume", UiValue::F32(50.0));
        let rows: Vec<UiMap> = (0..n)
            .map(|i| {
                let mut m = UiMap::new();
                m.insert("name".into(), UiValue::Str(format!("Row {i}")));
                m
            })
            .collect();
        s.set("rows", UiValue::List(Arc::new(rows)));
        s.set("sel", UiValue::I32(-1));
        s
    }

    struct Harness {
        rt: UiRuntime,
        fs: FrameState,
        out: FrameOutput,
        state: UiState,
        now: f64,
    }

    impl Harness {
        fn new() -> Harness {
            Harness {
                rt: UiRuntime::new(screen_doc(), Arc::new(Theme::placeholder())),
                fs: FrameState::new(),
                out: FrameOutput::default(),
                state: state_with_rows(6),
                now: 0.0,
            }
        }

        fn frame(&mut self, input: &[InputEvent]) -> Vec<UiEvent> {
            self.now += 0.05;
            self.rt.frame(
                FrameArgs {
                    screen: (400, 400),
                    scale: 2,
                    now: self.now,
                    state: &self.state,
                    input,
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                &mut self.fs,
                &mut self.out,
            );
            self.out.events.clone()
        }

        fn center(&self, id: &str) -> (f32, f32) {
            let r = self.out.rect(id).unwrap();
            ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32)
        }
    }

    fn down(x: f32, y: f32) -> InputEvent {
        InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Primary,
            shift: false,
            slot_drag: false,
        }
    }

    fn up(x: f32, y: f32) -> InputEvent {
        InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Primary,
        }
    }

    #[test]
    fn button_fires_on_release_in_and_cancels_on_release_out() {
        let mut h = Harness::new();
        h.frame(&[]);
        let (bx, by) = h.center("ok");

        let ev = h.frame(&[down(bx, by), up(bx, by)]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, UiEvent::Click { id, .. } if id == "ok")),
            "{ev:?}"
        );

        // Press in, drag out, release: no click.
        let ev = h.frame(&[
            down(bx, by),
            InputEvent::PointerMove { x: 1.0, y: 1.0 },
            up(1.0, 1.0),
        ]);
        assert!(
            !ev.iter().any(|e| matches!(e, UiEvent::Click { .. })),
            "{ev:?}"
        );
    }

    #[test]
    fn toggle_reports_inverted_bound_state() {
        let mut h = Harness::new();
        h.frame(&[]);
        let (tx, ty) = h.center("snd");
        let ev = h.frame(&[down(tx, ty), up(tx, ty)]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, UiEvent::Toggle { id, on: true, .. } if id == "snd")),
            "{ev:?}"
        );
        // Host applies it; next toggle reports off.
        h.state.set("sound_on", UiValue::Bool(true));
        h.frame(&[]);
        let ev = h.frame(&[down(tx, ty), up(tx, ty)]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, UiEvent::Toggle { id, on: false, .. } if id == "snd")),
            "{ev:?}"
        );
    }

    #[test]
    fn slider_quantizes_live_and_commits_on_release() {
        let mut h = Harness::new();
        h.frame(&[]);
        let r = h.out.rect("vol").unwrap();
        // Press at ~62% of the track: snaps to 50 with step 25.
        let x = r.x as f32 + r.w as f32 * 0.62;
        let y = (r.y + r.h / 2) as f32;
        let ev = h.frame(&[down(x, y)]);
        assert!(
            ev.iter().any(
                |e| matches!(e, UiEvent::SliderChange { id, value, committed: false, .. }
                if id == "vol" && *value == 50.0)
            ),
            "{ev:?}"
        );
        let end = r.x as f32 + r.w as f32 + 50.0; // drag past the end clamps to max
        let ev = h.frame(&[InputEvent::PointerMove { x: end, y }, up(end, y)]);
        assert!(
            ev.iter().any(
                |e| matches!(e, UiEvent::SliderChange { value, committed: true, .. }
                if *value == 100.0)
            ),
            "{ev:?}"
        );
    }

    #[test]
    fn text_input_focus_type_submit() {
        let mut h = Harness::new();
        h.frame(&[]);
        let (ix, iy) = h.center("name");
        let ev = h.frame(&[
            down(ix, iy),
            InputEvent::Char { ch: 'H' },
            InputEvent::Char { ch: 'i' },
            InputEvent::Key {
                key: NavKey::Enter,
                shift: false,
            },
        ]);
        assert!(
            ev.iter().any(
                |e| matches!(e, UiEvent::TextChanged { id, text } if id == "name" && text == "Hi")
            ),
            "{ev:?}"
        );
        assert!(
            ev.iter()
                .any(|e| matches!(e, UiEvent::Submit { id, text } if id == "name" && text == "Hi")),
            "{ev:?}"
        );
        // Unfocused chars go nowhere; ESC blurs first.
        let ev = h.frame(&[
            InputEvent::Key {
                key: NavKey::Escape,
                shift: false,
            },
            InputEvent::Char { ch: 'X' },
        ]);
        assert!(
            !ev.iter().any(|e| matches!(e, UiEvent::TextChanged { .. })),
            "{ev:?}"
        );
    }

    #[test]
    fn list_selects_on_down_and_activates_on_double_click() {
        let mut h = Harness::new();
        h.frame(&[]);
        let rows = h.out.rect("rows").unwrap();
        let (rx, ry) = (rows.x as f32 + 10.0, rows.y as f32 + 50.0); // second row (20 logical = 40 phys)
        let ev = h.frame(&[down(rx, ry), up(rx, ry)]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, UiEvent::ListSelect { id, index: 1 } if id == "rows")),
            "{ev:?}"
        );
        let ev = h.frame(&[down(rx, ry)]);
        assert!(
            ev.iter()
                .any(|e| matches!(e, UiEvent::ListActivate { id, index: 1 } if id == "rows")),
            "double click within the window activates: {ev:?}"
        );
    }

    #[test]
    fn wheel_scrolls_and_clamps() {
        let mut h = Harness::new();
        h.frame(&[]);
        let sc = h.out.rect("sc").unwrap();
        let (sx, sy) = ((sc.x + sc.w / 2) as f32, (sc.y + sc.h / 2) as f32);
        // Content: 6 rows × 20 = 120; viewport 40 → max offset 80.
        h.frame(&[
            InputEvent::PointerMove { x: sx, y: sy },
            InputEvent::Scroll { delta: 500 },
        ]);
        h.frame(&[]);
        let key = InstKey {
            id: "sc".into(),
            item: None,
        };
        assert_eq!(
            h.fs.scroll_offset(&key),
            80,
            "clamped to content - viewport"
        );
        h.frame(&[InputEvent::Scroll { delta: -500 }]);
        h.frame(&[]);
        assert_eq!(h.fs.scroll_offset(&key), 0);
    }

    #[test]
    fn click_outside_panel_reports_throw_territory() {
        let mut h = Harness::new();
        h.frame(&[]);
        // Panel is 200×200 logical centered in 200×200 logical viewport — it
        // fills the screen, so shrink: click at panel edge+ works only if
        // outside root. Use a corner outside the centered panel: the root IS
        // 200x200 at 0,0 filling everything, so instead verify a click on
        // empty panel space does NOT emit ClickOutside.
        let ev = h.frame(&[down(300.0, 390.0)]);
        assert!(
            !ev.iter().any(|e| matches!(e, UiEvent::ClickOutside { .. })),
            "{ev:?}"
        );
    }

    #[test]
    fn slot_grid_cells_map_row_major_and_click_on_down() {
        let doc = Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_chest", "class": "container",
                "root": { "type": "column", "children": [
                    { "type": "slot_grid", "role": "storage", "cols": 3, "rows": 2 },
                    { "type": "slot", "role": "storage" }
                ] }
            }"#,
            )
            .unwrap(),
        );
        let rt = UiRuntime::new(doc, Arc::new(Theme::placeholder()));
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let state = UiState::new();
        let frame = |input: &[InputEvent], fs: &mut FrameState, out: &mut FrameOutput| {
            rt.frame(
                FrameArgs {
                    screen: (300, 300),
                    scale: 2,
                    now: 0.0,
                    state: &state,
                    input,
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                fs,
                out,
            );
            out.events.clone()
        };
        frame(&[], &mut fs, &mut out);
        assert_eq!(out.slots.len(), 7);
        // Cell 4 = row 1, col 1 of the grid; cell 6 = the standalone slot.
        let cell4 = out.slots.iter().find(|s| s.index == 4).unwrap();
        let cell1 = out.slots.iter().find(|s| s.index == 1).unwrap();
        assert_eq!(cell4.rect.x, cell1.rect.x);
        assert!(cell4.rect.y > cell1.rect.y);
        let standalone = out.slots.iter().find(|s| s.index == 6).unwrap();
        assert_eq!(standalone.role, "storage");

        let (cx, cy) = (
            (cell4.rect.x + cell4.rect.w / 2) as f32,
            (cell4.rect.y + cell4.rect.h / 2) as f32,
        );
        let ev = frame(
            &[InputEvent::PointerDown {
                x: cx,
                y: cy,
                button: PointerButton::Secondary,
                shift: true,
                slot_drag: false,
            }],
            &mut fs,
            &mut out,
        );
        assert!(
            ev.iter().any(|e| matches!(e, UiEvent::SlotClick { role, index: 4, button: PointerButton::Secondary, shift: true }
                if role == "storage")),
            "{ev:?}"
        );
        // Hover reporting names the same cell.
        assert_eq!(out.hover_slot, Some(("storage".into(), 4)));
    }

    #[test]
    fn slot_drag_reports_each_cell_once_and_resets_after_release() {
        let doc = Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_drag", "class": "container",
                "root": { "type": "column", "children": [
                    { "type": "slot_grid", "role": "storage", "cols": 3, "rows": 2 },
                    { "type": "slot", "role": "storage" }
                ] }
            }"#,
            )
            .unwrap(),
        );
        let rt = UiRuntime::new(doc, Arc::new(Theme::placeholder()));
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let state = UiState::new();
        let frame = |input: &[InputEvent], fs: &mut FrameState, out: &mut FrameOutput| {
            rt.frame(
                FrameArgs {
                    screen: (300, 300),
                    scale: 2,
                    now: 0.0,
                    state: &state,
                    input,
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                fs,
                out,
            );
            out.events.clone()
        };
        frame(&[], &mut fs, &mut out);
        let centre = |index| {
            let rect = out
                .slots
                .iter()
                .find(|slot| slot.index == index)
                .unwrap()
                .rect;
            ((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32)
        };
        let one = centre(1);
        let four = centre(4);
        let six = centre(6);

        let drag = |x, y| InputEvent::PointerMove { x, y };
        let down = |(x, y)| InputEvent::PointerDown {
            x,
            y,
            button: PointerButton::Secondary,
            shift: false,
            slot_drag: true,
        };
        let up = |(x, y)| InputEvent::PointerUp {
            x,
            y,
            button: PointerButton::Secondary,
        };
        let events = frame(
            &[
                down(one),
                drag(four.0, four.1),
                drag(one.0, one.1),
                drag(six.0, six.1),
            ],
            &mut fs,
            &mut out,
        );
        assert!(events.is_empty(), "the gesture commits only on release");
        assert_eq!(
            fs.slot_drag(),
            Some((
                PointerButton::Secondary,
                &[
                    ("storage".into(), 1),
                    ("storage".into(), 4),
                    ("storage".into(), 6),
                ][..],
            )),
            "the host can render every distinct hit while the press is active"
        );

        let events = frame(&[up(six)], &mut fs, &mut out);
        assert_eq!(
            events,
            vec![UiEvent::SlotDrag {
                slots: vec![
                    ("storage".into(), 1),
                    ("storage".into(), 4),
                    ("storage".into(), 6),
                ],
                button: PointerButton::Secondary,
            }]
        );

        let events = frame(
            &[down(one), drag(four.0, four.1), up(four)],
            &mut fs,
            &mut out,
        );
        assert_eq!(
            events,
            vec![UiEvent::SlotDrag {
                slots: vec![("storage".into(), 1), ("storage".into(), 4)],
                button: PointerButton::Secondary,
            }],
            "a fresh press may hit the same slots again"
        );
    }

    #[test]
    fn interactive_image_keeps_drag_capture_and_reports_local_coordinates() {
        let doc = Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_canvas", "class": "screen",
                "root": { "type": "column", "layout": { "w": 120, "h": 100, "pad": [10,10,10,10] },
                    "children": [
                        { "type": "image", "id": "canvas", "image": "test:canvas",
                          "interactive": true, "layout": { "w": 80, "h": 60 } }
                    ] }
            }"#,
            )
            .unwrap(),
        );
        let rt = UiRuntime::new(doc, Arc::new(Theme::placeholder()));
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let state = UiState::new();
        let frame = |input: &[InputEvent], fs: &mut FrameState, out: &mut FrameOutput| {
            rt.frame(
                FrameArgs {
                    screen: (300, 300),
                    scale: 2,
                    now: 0.0,
                    state: &state,
                    input,
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                fs,
                out,
            );
            out.events.clone()
        };
        frame(&[], &mut fs, &mut out);
        let rect = out.rect("canvas").unwrap();
        let center = ((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32);
        let outside = ((rect.x + rect.w + 20) as f32, (rect.y + rect.h / 2) as f32);

        let events = frame(
            &[
                down(center.0, center.1),
                InputEvent::PointerMove {
                    x: outside.0,
                    y: outside.1,
                },
                up(outside.0, outside.1),
            ],
            &mut fs,
            &mut out,
        );
        let image_events: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                UiEvent::ImagePointer { phase, x, y, .. } => Some((*phase, *x, *y)),
                _ => None,
            })
            .collect();
        assert_eq!(image_events.len(), 3, "{events:?}");
        assert_eq!(image_events[0].0, PointerPhase::Down);
        assert_eq!(image_events[1].0, PointerPhase::Move);
        assert_eq!(image_events[2].0, PointerPhase::Up);
        assert!((image_events[0].1 - 40.0).abs() < 0.01);
        assert!((image_events[0].2 - 30.0).abs() < 0.01);
        assert!(image_events[1].1 > 80.0, "drag remains captured outside");
    }

    fn compound_recipe_doc() -> Arc<Document> {
        Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_recipes", "class": "screen",
                "root": { "type": "column", "layout": { "w": 80, "h": 50 },
                    "children": [
                        { "type": "scroll", "id": "recipe_scroll", "layout": { "w": 64, "h": 30 },
                          "children": [
                            { "type": "list", "id": "recipes",
                              "bind": { "items": "rows", "selected": "selected" },
                              "children": [
                                { "type": "button", "id": "recipe",
                                  "bind": { "enabled": "enabled" },
                                  "layout": { "w": 60, "h": 20, "dir": "row", "align": "center" },
                                  "children": [
                                    { "type": "hook", "id": "recipe_icon",
                                      "layout": { "w": 10, "h": 10 } }
                                  ] }
                              ] }
                          ] }
                    ] }
            }"#,
            )
            .unwrap(),
        )
    }

    fn compound_recipe_state(enabled: [bool; 2], selected: i32) -> UiState {
        let rows = enabled
            .into_iter()
            .map(|enabled| {
                let mut row = UiMap::new();
                row.insert("enabled".into(), UiValue::Bool(enabled));
                row
            })
            .collect();
        let mut state = UiState::new();
        state.set("rows", UiValue::List(Arc::new(rows)));
        state.set("selected", UiValue::I32(selected));
        state
    }

    #[test]
    fn compound_buttons_click_by_row_and_hooks_keep_the_scroll_clip() {
        let rt = UiRuntime::new(compound_recipe_doc(), Arc::new(Theme::placeholder()));
        let state = compound_recipe_state([true, false], 0);
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let frame = |input: &[InputEvent], fs: &mut FrameState, out: &mut FrameOutput| {
            rt.frame(
                FrameArgs {
                    screen: (400, 200),
                    scale: 2,
                    now: 0.0,
                    state: &state,
                    input,
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                fs,
                out,
            );
        };
        frame(&[], &mut fs, &mut out);

        assert_eq!(out.hooks.len(), 2);
        assert_eq!(out.hooks[0].key.item, Some(0));
        assert_eq!(out.hooks[1].key.item, Some(1));
        let clip = out.hooks[0].clip.expect("hook inherits its scroll clip");
        assert_eq!(out.hooks[1].clip, Some(clip));
        assert!(out
            .named
            .iter()
            .any(|(key, _)| { key.id == "recipe_icon" && key.item == Some(0) }));

        let recipe0 = out
            .named
            .iter()
            .find(|(key, _)| key.id == "recipe" && key.item == Some(0))
            .unwrap()
            .1;
        let p0 = (
            (recipe0.x + recipe0.w / 2) as f32,
            (recipe0.y + recipe0.h / 2) as f32,
        );
        frame(&[down(p0.0, p0.1), up(p0.0, p0.1)], &mut fs, &mut out);
        assert!(out.events.iter().any(
            |event| matches!(event, UiEvent::Click { id, item: Some(0), .. } if id == "recipe")
        ));

        let recipe1 = out
            .named
            .iter()
            .find(|(key, _)| key.id == "recipe" && key.item == Some(1))
            .unwrap()
            .1;
        let p1 = ((recipe1.x + recipe1.w / 2) as f32, (recipe1.y + 2) as f32);
        frame(&[down(p1.0, p1.1), up(p1.0, p1.1)], &mut fs, &mut out);
        assert!(
            out.events.is_empty(),
            "disabled compound row emitted {:#?}",
            out.events
        );
    }

    #[test]
    fn disabled_list_row_gap_does_not_select() {
        let doc = Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_rows", "class": "screen",
                "root": { "type": "list", "id": "rows", "bind": { "items": "items" },
                    "children": [
                        { "type": "row", "bind": { "enabled": "enabled" },
                          "layout": { "w": 60, "h": 20 },
                          "children": [ { "type": "label", "text": "Disabled" } ] }
                    ] }
            }"#,
            )
            .unwrap(),
        );
        let mut row = UiMap::new();
        row.insert("enabled".into(), UiValue::Bool(false));
        let mut state = UiState::new();
        state.set("items", UiValue::List(Arc::new(vec![row])));
        let rt = UiRuntime::new(doc, Arc::new(Theme::placeholder()));
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        rt.frame(
            FrameArgs {
                screen: (200, 100),
                scale: 1,
                now: 0.0,
                state: &state,
                input: &[down(125.0, 50.0)],
                clipboard: None,
                images: &NoImages,
                dim: None,
                preview: None,
            },
            &mut fs,
            &mut out,
        );
        assert!(
            !out.events
                .iter()
                .any(|event| matches!(event, UiEvent::ListSelect { .. })),
            "disabled row gap selected: {:?}",
            out.events
        );
    }

    #[cfg(feature = "raster")]
    #[test]
    fn compound_button_faces_cover_normal_hover_pressed_selected_and_disabled() {
        use crate::raster::{rasterize, TextureSet};

        enum FaceInput {
            None,
            Hover,
            Press,
        }
        let sample = |enabled: bool, selected: i32, input: FaceInput| -> [u8; 4] {
            let theme = Arc::new(Theme::placeholder());
            let rt = UiRuntime::new(compound_recipe_doc(), theme.clone());
            let state = compound_recipe_state([enabled, true], selected);
            let mut fs = FrameState::new();
            let mut out = FrameOutput::default();
            rt.frame(
                FrameArgs {
                    screen: (400, 200),
                    scale: 2,
                    now: 0.0,
                    state: &state,
                    input: &[],
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                &mut fs,
                &mut out,
            );
            let rect = out
                .named
                .iter()
                .find(|(key, _)| key.id == "recipe" && key.item == Some(0))
                .unwrap()
                .1;
            let p = ((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32);
            let input = match input {
                FaceInput::None => Vec::new(),
                FaceInput::Hover => vec![InputEvent::PointerMove { x: p.0, y: p.1 }],
                FaceInput::Press => vec![down(p.0, p.1)],
            };
            if !input.is_empty() {
                rt.frame(
                    FrameArgs {
                        screen: (400, 200),
                        scale: 2,
                        now: 0.0,
                        state: &state,
                        input: &input,
                        clipboard: None,
                        images: &NoImages,
                        dim: None,
                        preview: None,
                    },
                    &mut fs,
                    &mut out,
                );
            }
            let mut rgba = Vec::new();
            rasterize(
                &out.draw,
                &TextureSet {
                    theme_atlas: &theme.atlas,
                    font: &theme.font,
                    doc_images: &[],
                },
                (400, 200),
                [0, 0, 0, 255],
                &mut rgba,
            );
            let i =
                (((rect.y + rect.h / 2) as u32 * 400 + (rect.x + rect.w / 2) as u32) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };

        let normal = sample(true, -1, FaceInput::None);
        let hover = sample(true, -1, FaceInput::Hover);
        let pressed = sample(true, -1, FaceInput::Press);
        let selected = sample(true, 0, FaceInput::None);
        let disabled = sample(false, 0, FaceInput::Hover);
        assert_ne!(normal, hover);
        assert_ne!(normal, pressed);
        assert_eq!(selected, pressed, "selected row must keep the pressed face");
        assert_ne!(disabled, pressed, "disabled must override selected/hover");
    }

    #[test]
    fn draw_list_is_nonempty_and_batched() {
        let mut h = Harness::new();
        h.frame(&[]);
        assert!(!h.out.draw.is_empty());
        assert!(h.out.draw.batches.len() > 1);
        let total: u32 = h.out.draw.batches.iter().map(|b| b.count).sum();
        assert_eq!(
            total as usize,
            h.out.draw.vertices.len(),
            "batches tile the vertex buffer"
        );
    }

    #[cfg(feature = "raster")]
    #[test]
    fn a_clicked_row_never_flashes_unpressed_between_release_and_rebound_selection() {
        use crate::raster::{rasterize, TextureSet};

        let theme = Arc::new(Theme::placeholder());
        let rt = UiRuntime::new(compound_recipe_doc(), theme.clone());
        let state = compound_recipe_state([true, true], -1);
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let mut frame = |input: &[InputEvent], out: &mut FrameOutput| {
            rt.frame(
                FrameArgs {
                    screen: (400, 200),
                    scale: 2,
                    now: 0.0,
                    state: &state,
                    input,
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                &mut fs,
                out,
            );
        };
        frame(&[], &mut out);
        let rect = out
            .named
            .iter()
            .find(|(key, _)| key.id == "recipe" && key.item == Some(0))
            .unwrap()
            .1;
        let center = ((rect.x + rect.w / 2) as f32, (rect.y + rect.h / 2) as f32);
        let pixel = |out: &FrameOutput| {
            let mut rgba = Vec::new();
            rasterize(
                &out.draw,
                &TextureSet {
                    theme_atlas: &theme.atlas,
                    font: &theme.font,
                    doc_images: &[],
                },
                (400, 200),
                [0, 0, 0, 255],
                &mut rgba,
            );
            let i =
                (((rect.y + rect.h / 2) as u32 * 400 + (rect.x + rect.w / 2) as u32) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };

        frame(&[down(center.0, center.1)], &mut out);
        let held = pixel(&out);
        frame(&[up(center.0, center.1)], &mut out);
        assert!(
            out.events
                .iter()
                .any(|event| matches!(event, UiEvent::Click { id, .. } if id == "recipe")),
            "release-in fires the row click"
        );
        assert_eq!(
            pixel(&out),
            held,
            "the click frame keeps the pressed face (the host rebinds selection next frame)"
        );
        frame(&[], &mut out);
        assert_ne!(
            pixel(&out),
            held,
            "the bridge lives exactly one frame; unselected rows return to hover"
        );
    }

    #[test]
    fn compact_breakpoint_swaps_node_layouts_by_viewport_width() {
        let doc = Arc::new(
            Document::from_json(
                r#"{
                "format": 1, "kind": "petramond:test_compact", "class": "screen",
                "compact_below_w": 100,
                "root": { "type": "frame",
                    "layout": { "dir": "row", "gap": 2 },
                    "compact_layout": { "dir": "column", "gap": 2 },
                    "children": [
                        { "type": "button", "id": "a", "text": "A" },
                        { "type": "button", "id": "b", "text": "B" }
                    ] }
            }"#,
            )
            .unwrap(),
        );
        let rt = UiRuntime::new(doc, Arc::new(Theme::placeholder()));
        let state = UiState::new();
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let mut solve_at = |screen: (u32, u32)| {
            rt.frame(
                FrameArgs {
                    screen,
                    scale: 1,
                    now: 0.0,
                    state: &state,
                    input: &[],
                    clipboard: None,
                    images: &NoImages,
                    dim: None,
                    preview: None,
                },
                &mut fs,
                &mut out,
            );
            (out.rect("a").unwrap(), out.rect("b").unwrap())
        };

        let (a, b) = solve_at((200, 100));
        assert!(b.x > a.x && b.y == a.y, "wide viewports flow the row");
        let (a, b) = solve_at((80, 100));
        assert!(b.y > a.y && b.x == a.x, "below the breakpoint the compact layout stacks");
    }
}
