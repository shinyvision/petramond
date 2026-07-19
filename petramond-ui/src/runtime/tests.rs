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
    assert!(out
        .events
        .iter()
        .any(|event| matches!(event, UiEvent::Click { id, item: Some(0), .. } if id == "recipe")));

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
        let i = (((rect.y + rect.h / 2) as u32 * 400 + (rect.x + rect.w / 2) as u32) * 4) as usize;
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

fn tab_doc() -> Arc<Document> {
    Arc::new(
        Document::from_json(
            r#"{
                "format": 1, "kind": "petramond:tab_screen", "class": "screen",
                "root": { "type": "column", "layout": { "w": 200, "h": 100, "pad": [10,10,10,10], "gap": 4 },
                    "children": [
                        { "type": "tab_bar", "id": "tabs",
                          "tabs": [ { "key": "world", "label": "WORLD" },
                                    { "key": "mods", "label": "MODS" } ],
                          "bind": { "selected": "tab", "enabled": "tabs_on" } },
                        { "type": "label", "text": "world page", "bind": { "visible": "page_world" } },
                        { "type": "label", "text": "mods page", "bind": { "visible": "page_mods" } }
                    ] }
            }"#,
        )
        .unwrap(),
    )
}

/// The physical center of tab cell `i` (the bar solves at scale 2).
fn tab_center(out: &FrameOutput, theme: &Theme, doc: &Document, i: usize) -> (f32, f32) {
    let phys = out.rect("tabs").unwrap();
    let logical = RectI {
        x: phys.x / 2,
        y: phys.y / 2,
        w: phys.w / 2,
        h: phys.h / 2,
    };
    let NodeKind::TabBar { tabs } = &doc.root.children[0].kind else {
        panic!("first child is the tab bar");
    };
    let widths = crate::widget::tab_widths(theme, tabs);
    let cell = crate::widget::tab_cell(logical, &widths, theme.metrics.tab_gap, i);
    (
        (cell.x + cell.w / 2) as f32 * 2.0,
        (cell.y + cell.h / 2) as f32 * 2.0,
    )
}

#[test]
fn tab_bar_fires_on_down_and_respects_enabled_and_gaps() {
    let doc = tab_doc();
    let theme = Arc::new(Theme::placeholder());
    let rt = UiRuntime::new(doc.clone(), theme.clone());
    let mut fs = FrameState::new();
    let mut out = FrameOutput::default();
    let mut state = UiState::new();
    state.set("tab", UiValue::I32(0));
    let mut frame =
        |state: &UiState, fs: &mut FrameState, out: &mut FrameOutput, input: &[InputEvent]| {
            rt.frame(
                FrameArgs {
                    screen: (400, 200),
                    scale: 2,
                    now: 0.0,
                    state,
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

    frame(&state, &mut fs, &mut out, &[]);
    let (mx, my) = tab_center(&out, &theme, &doc, 1);

    // Fires on pointer DOWN, before any release.
    let ev = frame(&state, &mut fs, &mut out, &[down(mx, my)]);
    assert_eq!(
        ev,
        vec![UiEvent::TabSelect {
            id: "tabs".into(),
            index: 1
        }]
    );
    assert_eq!(frame(&state, &mut fs, &mut out, &[up(mx, my)]), vec![]);

    // The gap between cells hits nothing.
    let (wx, _) = tab_center(&out, &theme, &doc, 0);
    let bar = out.rect("tabs").unwrap();
    let NodeKind::TabBar { tabs } = &doc.root.children[0].kind else {
        unreachable!()
    };
    let w0 = crate::widget::tab_widths(&theme, tabs)[0];
    let gap_x = bar.x as f32 + (w0 as f32 + theme.metrics.tab_gap as f32 / 2.0) * 2.0;
    let ev = frame(&state, &mut fs, &mut out, &[down(gap_x, my), up(gap_x, my)]);
    assert_eq!(ev, vec![], "gap between tabs is inert (x={gap_x}, w0={w0})");
    let _ = wx;

    // A disabled bar fires nothing.
    state.set("tabs_on", UiValue::Bool(false));
    let ev = frame(&state, &mut fs, &mut out, &[down(mx, my), up(mx, my)]);
    assert_eq!(ev, vec![]);
}

#[cfg(feature = "raster")]
#[test]
fn tab_faces_track_bound_selection_and_hover() {
    use crate::raster::{rasterize, TextureSet};

    let doc = tab_doc();
    let theme = Arc::new(Theme::placeholder());
    let sample = |selected: i32, hover_tab: Option<usize>| -> [u8; 4] {
        let rt = UiRuntime::new(doc.clone(), theme.clone());
        let mut fs = FrameState::new();
        let mut out = FrameOutput::default();
        let mut state = UiState::new();
        state.set("tab", UiValue::I32(selected));
        let mut run = |input: &[InputEvent], fs: &mut FrameState, out: &mut FrameOutput| {
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
        run(&[], &mut fs, &mut out);
        if let Some(t) = hover_tab {
            let (hx, hy) = tab_center(&out, &theme, &doc, t);
            run(
                &[InputEvent::PointerMove { x: hx, y: hy }],
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
        // Sample inside tab 0's fill, clear of the border and the label rows.
        let (cx, _) = tab_center(&out, &theme, &doc, 0);
        let bar = out.rect("tabs").unwrap();
        let (px, py) = (cx as u32, (bar.y + 10) as u32);
        let i = ((py * 400 + px) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    };

    let default = sample(1, None);
    let selected = sample(0, None);
    let hovered = sample(1, Some(0));
    assert_ne!(default, selected, "bound selection changes the tab face");
    assert_ne!(default, hovered, "cell-level hover changes the tab face");
    assert_ne!(selected, hovered);
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
        let i = (((rect.y + rect.h / 2) as u32 * 400 + (rect.x + rect.w / 2) as u32) * 4) as usize;
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
    assert!(
        b.y > a.y && b.x == a.x,
        "below the breakpoint the compact layout stacks"
    );
}
