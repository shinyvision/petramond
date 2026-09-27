use super::*;
use crate::doc::{Document, Node};
use crate::state::{UiState, UiValue};
use crate::tree::InstTree;

struct MockEnv;
impl LayoutEnv for MockEnv {
    fn leaf_size(
        &self,
        node: &Node,
        text: Option<&str>,
        _image: Option<&str>,
        avail_w: Option<i32>,
    ) -> (i32, i32) {
        let text_len = text.map(|t| t.chars().count() as i32).unwrap_or(0);
        match &node.kind {
            NodeKind::Label { wrap, .. } => {
                let w = text_len * 6;
                match (wrap, avail_w) {
                    (true, Some(avail)) if avail > 0 && w > avail => {
                        let per_line = (avail / 6).max(1);
                        let lines = (text_len + per_line - 1) / per_line;
                        (per_line * 6, lines * 9)
                    }
                    _ => (w, 9),
                }
            }
            NodeKind::Button { .. } => (text_len * 6 + 8, 20),
            NodeKind::Checkbox => (10, 10),
            NodeKind::Toggle { .. } => (18, 10),
            NodeKind::SlotGrid { cols, rows, .. } => {
                let m = self.slot_metrics();
                (
                    *cols as i32 * m.slot + (*cols as i32 - 1) * m.gap,
                    *rows as i32 * m.slot + (*rows as i32 - 1) * m.gap,
                )
            }
            NodeKind::Slot { .. } => {
                let m = self.slot_metrics();
                (m.slot, m.slot)
            }
            _ => (0, 0),
        }
    }
    fn slot_metrics(&self) -> SlotMetrics {
        SlotMetrics { slot: 18, gap: 0 }
    }
}

fn solve_doc(json: &str, viewport: (i32, i32)) -> (Solved, Document) {
    let doc = Document::from_json(json).unwrap();
    let state = UiState::new();
    let tree = InstTree::expand(&doc, &state);
    let solved = solve(&tree, &MockEnv, viewport, &|_| 0);
    (solved, doc)
}

#[test]
fn column_pad_gap_and_centering() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column",
                "layout": { "pad": [8,6,8,6], "gap": 4 },
                "children": [
                    { "type": "checkbox", "id": "a" },
                    { "type": "toggle", "id": "b" }
                ] }
        }"#,
        (200, 100),
    );
    assert_eq!(
        s.rects[0],
        RectI {
            x: 83,
            y: 32,
            w: 34,
            h: 36
        }
    );
    assert_eq!(
        s.rects[1],
        RectI {
            x: 91,
            y: 38,
            w: 10,
            h: 10
        }
    );
    assert_eq!(
        s.rects[2],
        RectI {
            x: 91,
            y: 52,
            w: 18,
            h: 10
        }
    );
}

#[test]
fn grow_distributes_leftover_with_remainder_to_first() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "row", "layout": { "w": 103, "h": 20 },
                "children": [
                    { "type": "spacer", "id": "a", "layout": { "w": { "grow": 1 } } },
                    { "type": "spacer", "id": "b", "layout": { "w": { "grow": 2 } } }
                ] }
        }"#,
        (200, 100),
    );
    assert_eq!(s.rects[1].w, 35);
    assert_eq!(s.rects[2].w, 68);
    assert_eq!(s.rects[1].w + s.rects[2].w, 103, "shares sum exactly");
    assert_eq!(s.rects[2].x, s.rects[1].x + s.rects[1].w);
}

#[test]
fn justify_and_align_position_children() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "row",
                "layout": { "w": 100, "h": 40, "justify": "space_between", "align": "center" },
                "children": [
                    { "type": "checkbox", "id": "a" },
                    { "type": "checkbox", "id": "b" },
                    { "type": "checkbox", "id": "c" }
                ] }
        }"#,
        (100, 40),
    );
    assert_eq!(s.rects[1].x, 0);
    assert_eq!(s.rects[2].x, 45);
    assert_eq!(s.rects[3].x, 90);
    assert!(s.rects[1..].iter().all(|r| r.y == 15));
}

#[test]
fn stretch_fills_cross_axis() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": 120, "h": 60, "align": "stretch" },
                "children": [ { "type": "button", "id": "ok", "text": "OK" } ] }
        }"#,
        (200, 100),
    );
    assert_eq!(s.rects[1].w, 120, "stretch fills the column width");
    assert_eq!(s.rects[1].h, 20, "main axis stays natural");
}

#[test]
fn leaf_button_keeps_leaf_size_while_compound_button_measures_children() {
    let (leaf, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "button", "id": "leaf", "text": "OK" }
        }"#,
        (100, 100),
    );
    assert_eq!(leaf.rects[0].w, 20);
    assert_eq!(leaf.rects[0].h, 20);

    let (compound, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "button", "id": "compound", "children": [
                { "type": "label", "text": "OK" }
            ] }
        }"#,
        (100, 100),
    );
    assert_eq!(compound.rects[0].w, 12);
    assert_eq!(compound.rects[0].h, 9);
    assert_eq!(compound.rects[1].w, 12);
    assert_eq!(compound.rects[1].h, 9);
}

#[test]
fn abs_children_leave_the_flow() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame", "layout": { "w": 100, "h": 100, "pad": [10,10,10,10] },
                "children": [
                    { "type": "checkbox", "id": "flow" },
                    { "type": "checkbox", "id": "deco", "layout": { "abs": { "x": 5, "y": 7 } } }
                ] }
        }"#,
        (100, 100),
    );
    assert_eq!(
        s.rects[1],
        RectI {
            x: 10,
            y: 10,
            w: 10,
            h: 10
        }
    );
    assert_eq!(
        s.rects[2],
        RectI {
            x: 15,
            y: 17,
            w: 10,
            h: 10
        }
    );
}

#[test]
fn bound_abs_position_overrides_the_authored_one_per_axis() {
    let json = r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "frame", "layout": { "w": 100, "h": 100 },
            "children": [
                { "type": "checkbox", "id": "deco",
                  "layout": { "abs": { "x": 5, "y": 7 } },
                  "bind": { "abs_x": "px", "abs_y": "py" } }
            ] }
    }"#;
    let doc = Document::from_json(json).unwrap();
    let solve_with = |entries: &[(&str, i32)]| {
        let mut state = UiState::new();
        for (k, v) in entries {
            state.set(*k, UiValue::I32(*v));
        }
        let tree = InstTree::expand(&doc, &state);
        solve(&tree, &MockEnv, (100, 100), &|_| 0)
    };
    let s = solve_with(&[]);
    assert_eq!((s.rects[1].x, s.rects[1].y), (5, 7));
    let s = solve_with(&[("px", 40), ("py", 60)]);
    assert_eq!((s.rects[1].x, s.rects[1].y), (40, 60));
    let s = solve_with(&[("py", 33)]);
    assert_eq!((s.rects[1].x, s.rects[1].y), (5, 33));
}

#[test]
fn a_bound_min_w_grows_the_parent_and_never_shrinks_below_the_authored_floor() {
    let json = r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "frame", "layout": { "w": 200, "h": 100 },
            "children": [
                { "type": "column", "id": "panel", "layout": { "align": "stretch" },
                  "children": [
                    { "type": "hook", "id": "strip",
                      "layout": { "w": { "grow": 1 }, "min_w": 40, "h": 12 },
                      "bind": { "min_w": "strip_w" } }
                  ] }
            ] }
    }"#;
    let doc = Document::from_json(json).unwrap();
    let width_with = |published: Option<i32>| {
        let mut state = UiState::new();
        if let Some(v) = published {
            state.set("strip_w", UiValue::I32(v));
        }
        let tree = InstTree::expand(&doc, &state);
        let s = solve(&tree, &MockEnv, (200, 100), &|_| 0);
        (s.rects[1].w, s.rects[2].w)
    };

    assert_eq!(width_with(None), (40, 40));
    assert_eq!(width_with(Some(120)), (120, 120));
    assert_eq!(width_with(Some(10)), (40, 40));
}

#[test]
fn an_overlay_node_is_raised_but_still_hit_testable() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame", "layout": { "w": 100, "h": 100 },
                "children": [
                    { "type": "checkbox", "id": "plain" },
                    { "type": "checkbox", "id": "on_top", "overlay": true,
                      "layout": { "abs": { "x": 5, "y": 5 } } }
                ] }
        }"#,
        (100, 100),
    );
    assert!(!s.raised[1] && !s.overlay[1], "ordinary node: base tier");
    assert!(s.raised[2], "flagged node paints in the raised tier");
    assert!(
        !s.overlay[2],
        "raised is NOT the tooltip flag — the node stays hit-testable"
    );
    assert!(s.hit(2, 6, 6), "the raised widget still takes the pointer");
}

#[test]
fn abs_grow_children_fill_parent_content() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame", "layout": { "w": 100, "h": 80, "pad": [10,6,14,8] },
                "children": [
                    { "type": "checkbox", "id": "bg", "layout": {
                        "w": { "grow": 1 }, "h": { "grow": 1 }, "abs": { "x": 3, "y": 4 }
                    } },
                    { "type": "checkbox", "id": "flow" }
                ] }
        }"#,
        (100, 80),
    );
    assert_eq!(
        s.rects[1],
        RectI {
            x: 13,
            y: 10,
            w: 73,
            h: 62
        }
    );
    assert_eq!(
        s.rects[2],
        RectI {
            x: 10,
            y: 6,
            w: 10,
            h: 10
        },
        "absolute decoration still leaves normal flow alone"
    );
}

#[test]
fn scroll_shifts_clips_and_reports_content() {
    let doc = Document::from_json(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "scroll", "id": "sc", "layout": { "w": 50, "h": 30, "gap": 2 },
                "children": [
                    { "type": "checkbox", "id": "a" },
                    { "type": "checkbox", "id": "b" },
                    { "type": "checkbox", "id": "c" }
                ] }
        }"#,
    )
    .unwrap();
    let state = UiState::new();
    let tree = InstTree::expand(&doc, &state);
    let solved = solve(&tree, &MockEnv, (50, 30), &|_| 8);
    assert_eq!(solved.scroll_content[0], Some((42, 34)));
    assert_eq!(solved.rects[1].w, 42, "rows reserve the scrollbar lane");
    assert_eq!(solved.rects[1].y, solved.rects[0].y - 8);
    let clip = solved.clips[1].expect("scroll children are clipped");
    assert_eq!(
        clip,
        RectI {
            x: 0,
            y: 0,
            w: 50,
            h: 30
        }
    );
    assert!(
        !solved.hit(1, 45, 28),
        "row scrolled partly out doesn't hit below clip"
    );
    assert!(solved.hit(2, 5, solved.rects[2].y), "visible row hits");
}

#[test]
fn grow_children_shrink_before_anything_overflows() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": 80, "h": 60 }, "children": [
                { "type": "label", "text": "hey" },
                { "type": "scroll", "id": "sc", "layout": { "h": { "grow": 1 }, "min_h": 12, "gap": 2 },
                  "children": [
                    { "type": "checkbox", "id": "a" },
                    { "type": "checkbox", "id": "b" },
                    { "type": "checkbox", "id": "c" }
                ] },
                { "type": "button", "id": "ok", "text": "OK" }
            ] }
        }"#,
        (80, 60),
    );
    assert_eq!(s.rects[2].h, 31, "scroll shrank by the 3px deficit");
    let button = s.rects[6];
    assert_eq!(
        button.y + button.h,
        s.rects[0].y + 60,
        "the button still ends inside the panel"
    );
    assert!(
        s.scroll_content[2].unwrap().1 > s.rects[2].h,
        "the shrunk scroll now overflows internally (scrollbar territory)"
    );
}

#[test]
fn shrink_stops_at_min_and_the_rest_overflows() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": 80, "h": 30 }, "children": [
                { "type": "scroll", "id": "sc", "layout": { "h": { "grow": 1 }, "min_h": 20, "gap": 2 },
                  "children": [
                    { "type": "checkbox", "id": "a" },
                    { "type": "checkbox", "id": "b" },
                    { "type": "checkbox", "id": "c" }
                ] },
                { "type": "button", "id": "ok", "text": "OK" }
            ] }
        }"#,
        (80, 30),
    );
    assert_eq!(s.rects[1].h, 20, "scroll clamps at min_h");
    let button = s.rects[5];
    assert!(
        button.y + button.h > s.rects[0].y + 30,
        "beyond every minimum, content overflows (last resort)"
    );
}

#[test]
fn two_growers_shrink_by_weight() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "row", "layout": { "w": 70, "h": 10 }, "children": [
                { "type": "spacer", "id": "a", "layout": { "w": { "grow": 1 }, "min_w": 10 } },
                { "type": "spacer", "id": "b", "layout": { "w": { "grow": 2 }, "min_w": 10 } }
            ] }
        }"#,
        (200, 100),
    );
    assert_eq!(s.rects[1].w + s.rects[2].w, 70);
}

#[test]
fn fitting_scroll_content_reserves_no_scrollbar_lane() {
    let doc = Document::from_json(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "scroll", "id": "sc", "layout": { "w": 50, "h": 40, "gap": 2 },
                "children": [
                    { "type": "checkbox", "id": "a" },
                    { "type": "checkbox", "id": "b" }
                ] }
        }"#,
    )
    .unwrap();
    let state = UiState::new();
    let tree = InstTree::expand(&doc, &state);
    let solved = solve(&tree, &MockEnv, (50, 40), &|_| 0);
    assert_eq!(solved.rects[1].w, 50);
}

#[test]
fn reserved_scrollbar_keeps_children_at_the_same_width_before_and_after_overflow() {
    fn child_width(children: usize) -> i32 {
        let mut nodes = String::new();
        for i in 0..children {
            nodes.push_str(&format!(r#"{{"type":"checkbox","id":"item{i}"}},"#));
        }
        nodes.pop();
        let doc = Document::from_json(&format!(
            r#"{{
                "format": 1, "kind": "petramond:x", "class": "screen",
                "root": {{ "type": "scroll", "axis": "vertical",
                    "layout": {{ "w": 50, "h": 40, "gap": 2, "reserve_scrollbar": true }},
                    "children": [{nodes}] }}
            }}"#,
        ))
        .unwrap();
        let tree = InstTree::expand(&doc, &UiState::new());
        solve(&tree, &MockEnv, (50, 40), &|_| 0).rects[1].w
    }

    assert_eq!(child_width(2), child_width(5));
    assert_eq!(child_width(2), 50 - MockEnv.scrollbar_width());
}

#[test]
fn reserved_horizontal_scrollbar_is_included_in_auto_height() {
    let (solved, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "scroll", "axis": "horizontal",
                "layout": { "w": 50, "reserve_scrollbar": true },
                "children": [ { "type": "checkbox" } ] }
        }"#,
        (50, 40),
    );
    assert_eq!(
        solved.rects[0].h,
        solved.rects[1].h + MockEnv.scrollbar_width()
    );
}

#[test]
fn wrapping_label_uses_column_width_hint() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": 66, "pad": [3,0,3,0] },
                "children": [
                    { "type": "label", "text": "hello world!", "wrap": true }
                ] }
        }"#,
        (200, 100),
    );
    assert_eq!(s.rects[1].h, 18);
    assert_eq!(s.rects[1].w, 60);
}

#[test]
fn a_wrapping_label_in_a_row_makes_the_row_as_tall_as_its_lines() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": 100, "align": "stretch" },
                "children": [
                    { "type": "row", "children": [
                        { "type": "label", "text": "a line far too long", "wrap": true,
                          "layout": { "w": { "grow": 1 }, "min_w": 0 } },
                        { "type": "button", "id": "b", "text": "Go" }
                    ] },
                    { "type": "label", "text": "under" }
                ] }
        }"#,
        (200, 100),
    );
    let (row, label, button, under) = (s.rects[1], s.rects[2], s.rects[3], s.rects[4]);
    assert_eq!((button.w, label.w), (20, 80));
    assert_eq!(row.h, 20, "the button is the taller: {row:?}");
    assert!(under.y >= row.y + row.h, "{under:?} overlaps {row:?}");

    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": 60, "align": "stretch" },
                "children": [
                    { "type": "row", "children": [
                        { "type": "label", "text": "a line far too long for this", "wrap": true,
                          "layout": { "w": { "grow": 1 }, "min_w": 0 } },
                        { "type": "button", "id": "b", "text": "Go" }
                    ] },
                    { "type": "label", "text": "under" }
                ] }
        }"#,
        (200, 100),
    );
    let (row, label, under) = (s.rects[1], s.rects[2], s.rects[4]);
    assert_eq!(label.w, 40);
    assert_eq!(row.h, 45, "{row:?}");
    assert!(under.y >= row.y + row.h, "{under:?} overlaps {row:?}");
}

#[test]
fn slot_grid_natural_size_and_row_major_cells() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "container",
            "root": { "type": "frame", "children": [
                { "type": "slot_grid", "id": "g", "role": "storage", "cols": 9, "rows": 3 }
            ] }
        }"#,
        (400, 300),
    );
    let g = s.rects[1];
    assert_eq!((g.w, g.h), (162, 54));
    let m = MockEnv.slot_metrics();
    assert_eq!(
        grid_cell(g, 9, 0, m),
        RectI {
            x: g.x,
            y: g.y,
            w: 18,
            h: 18
        }
    );
    assert_eq!(
        grid_cell(g, 9, 8, m),
        RectI {
            x: g.x + 8 * 18,
            y: g.y,
            w: 18,
            h: 18
        }
    );
    assert_eq!(
        grid_cell(g, 9, 9, m),
        RectI {
            x: g.x,
            y: g.y + 18,
            w: 18,
            h: 18
        }
    );
}

#[test]
fn root_anchor_end_with_margin_is_the_hotbar_rule() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:hotbar", "class": "hud",
            "root": { "type": "row", "layout": { "margin": [0,0,0,1], "anchor": { "h": "center", "v": "end" } },
                "children": [ { "type": "slot_grid", "role": "hotbar", "cols": 9, "rows": 1 } ] }
        }"#,
        (320, 240),
    );
    assert_eq!(
        s.rects[0].y,
        240 - 18 - 1,
        "pinned to bottom edge with 1px lift"
    );
    assert_eq!(s.rects[0].x, (320 - 162) / 2);
}

#[test]
fn solving_twice_is_identical() {
    let json = r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "layout": { "w": { "grow": 1 }, "h": { "grow": 1 }, "gap": 3 },
                "children": [
                    { "type": "label", "text": "abc" },
                    { "type": "row", "layout": { "gap": 5, "justify": "center" }, "children": [
                        { "type": "button", "id": "x", "text": "X" },
                        { "type": "spacer", "layout": { "w": { "grow": 3 } } },
                        { "type": "button", "id": "y", "text": "Y" }
                    ] },
                    { "type": "spacer", "layout": { "h": { "grow": 1 } } }
                ] }
        }"#;
    let doc = Document::from_json(json).unwrap();
    let mut state = UiState::new();
    state.set("irrelevant", UiValue::I32(1));
    let t1 = InstTree::expand(&doc, &state);
    let t2 = InstTree::expand(&doc, &state);
    let s1 = solve(&t1, &MockEnv, (517, 331), &|_| 0);
    let s2 = solve(&t2, &MockEnv, (517, 331), &|_| 0);
    assert_eq!(s1.rects, s2.rects);
    assert_eq!(s1.clips, s2.clips);
}

#[test]
fn min_max_clamps_apply() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "row", "layout": { "w": 300, "h": 20 }, "children": [
                { "type": "spacer", "id": "capped", "layout": { "w": { "grow": 1 }, "max_w": 40 } },
                { "type": "checkbox", "id": "padded", "layout": { "min_w": 25 } }
            ] }
        }"#,
        (300, 100),
    );
    assert_eq!(s.rects[1].w, 40, "grow capped by max_w");
    assert_eq!(s.rects[2].w, 25, "natural raised to min_w");
}

fn solve_rows(json: &str, viewport: (i32, i32), items: usize) -> Solved {
    let doc = Document::from_json(json).unwrap();
    let mut state = UiState::new();
    let rows: Vec<crate::state::UiMap> = (0..items).map(|_| crate::state::UiMap::new()).collect();
    state.set("rows", UiValue::List(std::sync::Arc::new(rows)));
    let tree = InstTree::expand(&doc, &state);
    solve(&tree, &MockEnv, viewport, &|_| 0)
}

const GRID_DOC: &str = r#"{
    "format": 1, "kind": "petramond:x", "class": "screen",
    "root": { "type": "column", "layout": { "w": 100, "h": 100 }, "children": [
        { "type": "list", "id": "grid", "cols": 4,
          "layout": { "w": { "grow": 1 }, "gap": 2 },
          "bind": { "items": "rows" },
          "children": [
            { "type": "hook", "id": "cell", "layout": { "w": 16, "h": 16 } }
          ] }
    ] }
}"#;

#[test]
fn grid_list_splits_columns_exactly_and_wraps_row_major() {
    let s = solve_rows(GRID_DOC, (200, 200), 6);
    let cells = &s.rects[2..];
    assert_eq!(cells.len(), 6);
    let widths: Vec<i32> = cells[..4].iter().map(|r| r.w).collect();
    assert_eq!(widths, vec![24, 24, 23, 23]);
    let xs: Vec<i32> = cells[..4].iter().map(|r| r.x - cells[0].x).collect();
    assert_eq!(xs, vec![0, 26, 52, 77], "cells + gaps tile the row exactly");
    let last = cells[3];
    assert_eq!(
        last.x + last.w - cells[0].x,
        100,
        "the grid fills its content width with no drift"
    );

    assert_eq!(cells[4].y - cells[0].y, 18);
    assert_eq!(cells[4].x, cells[0].x, "row-major wrap returns to column 0");
    assert_eq!(cells[5].x, cells[1].x);
    assert_eq!(
        s.rects[1].h, 34,
        "natural height is 2 rows of 16 plus a gap"
    );
}

#[test]
fn a_partial_last_row_does_not_stretch_its_cells() {
    let s = solve_rows(GRID_DOC, (200, 200), 5);
    let cells = &s.rects[2..];
    assert_eq!(cells.len(), 5);
    assert_eq!(
        cells[4].w, cells[0].w,
        "the lone last-row cell keeps its column width"
    );
    assert_eq!(cells[4].x, cells[0].x);
}

#[test]
fn tooltips_leave_the_flow_at_natural_size_and_unclipped() {
    let json = r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "scroll", "id": "sc", "layout": { "w": 100, "h": 40 }, "children": [
            { "type": "checkbox", "id": "a" },
            { "type": "tooltip", "id": "tip", "bind": { "visible": "show" },
              "layout": { "w": 60, "h": 20 },
              "children": [ { "type": "checkbox", "id": "inner" } ] },
            { "type": "checkbox", "id": "b" }
        ] }
    }"#;
    let doc = Document::from_json(json).unwrap();
    let state = UiState::new();
    let tree = InstTree::expand(&doc, &state);
    let s = solve(&tree, &MockEnv, (200, 200), &|_| 0);

    let (a, tip, b) = (s.rects[1], s.rects[2], s.rects[4]);
    assert_eq!(
        b.y - a.y,
        10,
        "the tooltip takes no space between its siblings"
    );
    assert_eq!((tip.w, tip.h), (60, 20), "tooltip arranges at its own size");

    assert!(s.clips[1].is_some() && s.clips[4].is_some());
    assert_eq!(s.clips[2], None);
    assert_eq!(s.clips[3], None, "the clip exemption covers the subtree");

    assert!(!s.overlay[1] && !s.overlay[4]);
    assert!(s.overlay[2] && s.overlay[3], "the whole subtree is overlay");
}

#[test]
fn a_wrap_label_inside_a_max_bounded_tooltip_wraps_at_the_cap() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "container",
            "root": { "type": "frame", "children": [
                { "type": "tooltip", "id": "tip", "bind": { "visible": "show" },
                  "layout": { "max_w": 60, "abs": { "x": 4, "y": 4 } },
                  "children": [
                      { "type": "label", "id": "t", "wrap": true,
                        "text": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" }
                  ] }
            ] }
        }"#,
        (500, 500),
    );
    let tip = s.rects[1];
    assert_eq!(tip.w, 60, "natural width clamps to max_w");
    assert_eq!(tip.h, 3 * 9, "the label wraps at the cap");

    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "container",
            "root": { "type": "frame", "children": [
                { "type": "tooltip", "id": "tip", "bind": { "visible": "show" },
                  "layout": { "max_w": 60, "abs": { "x": 4, "y": 4 } },
                  "children": [
                      { "type": "label", "id": "t", "wrap": true, "text": "aaaaa" }
                  ] }
            ] }
        }"#,
        (500, 500),
    );
    assert_eq!((s.rects[1].w, s.rects[1].h), (5 * 6, 9));
}

#[test]
fn bound_text_gives_back_width_before_a_row_pushes_its_widgets_out() {
    let json = r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "row", "id": "row",
            "layout": { "w": 100, "h": 20, "gap": 4, "align": "center" },
            "children": [
                { "type": "checkbox", "id": "icon" },
                { "type": "column", "id": "text", "children": [
                    { "type": "label", "id": "name", "bind": { "text": "name" } },
                    { "type": "label", "id": "desc", "bind": { "text": "desc" } }
                ] },
                { "type": "spacer", "layout": { "w": { "grow": 1 } } },
                { "type": "toggle", "id": "on" }
            ] }
    }"#;
    let doc = Document::from_json(json).unwrap();
    let mut state = UiState::new();
    state.set("name", UiValue::Str("Furniture".into()));
    state.set(
        "desc",
        UiValue::Str("A craftable chair, chains, and a cauldron".into()),
    );
    let tree = InstTree::expand(&doc, &state);
    let s = solve(&tree, &MockEnv, (200, 200), &|_| 0);

    let (row, toggle) = (s.rects[0], s.rects[6]);
    assert_eq!(row.w, 100, "the row keeps its authored width");
    assert!(
        toggle.x + toggle.w <= row.x + row.w,
        "toggle at {}..{} left the row {}..{}",
        toggle.x,
        toggle.x + toggle.w,
        row.x,
        row.x + row.w
    );
    for (i, id) in [(3u32, "name"), (4, "desc")] {
        let label = s.rects[i as usize];
        assert!(
            label.x + label.w <= row.x + row.w,
            "{id} at {}..{} left the row",
            label.x,
            label.x + label.w
        );
    }
}

#[test]
fn authored_text_keeps_its_width_while_bound_text_beside_it_shrinks() {
    let json = r#"{
        "format": 1, "kind": "petramond:x", "class": "screen",
        "root": { "type": "row", "layout": { "w": 60, "h": 10 }, "children": [
            { "type": "label", "id": "caption", "text": "Seed" },
            { "type": "label", "id": "value", "bind": { "text": "seed" } }
        ] }
    }"#;
    let doc = Document::from_json(json).unwrap();
    let mut state = UiState::new();
    state.set("seed", UiValue::Str("-8149203114772265771".into()));
    let tree = InstTree::expand(&doc, &state);
    let s = solve(&tree, &MockEnv, (200, 200), &|_| 0);

    assert_eq!(s.rects[1].w, 24, "the authored caption keeps all 4 glyphs");
    assert_eq!(
        s.rects[2].w, 36,
        "the bound value absorbs the whole deficit"
    );
}

#[test]
fn an_auto_panel_gives_height_back_through_the_grower_inside_it() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame", "id": "screen",
                "layout": { "w": { "grow": 1 }, "h": { "grow": 1 },
                            "align": "center", "justify": "center" },
                "children": [
                    { "type": "column", "id": "panel", "layout": { "w": 80, "gap": 2 }, "children": [
                        { "type": "label", "text": "Controls" },
                        { "type": "scroll", "id": "list", "layout": { "h": { "grow": 1 }, "min_h": 12 },
                          "children": [
                            { "type": "checkbox", "id": "a" },
                            { "type": "checkbox", "id": "b" },
                            { "type": "checkbox", "id": "c" },
                            { "type": "checkbox", "id": "d" }
                        ] },
                        { "type": "button", "id": "back", "text": "Back" }
                    ] }
                ] }
        }"#,
        (80, 60),
    );
    let (screen, panel, back) = (s.rects[0], s.rects[1], s.rects[8]);
    assert_eq!(panel.h, 60, "the panel took the viewport's height, not 73");
    assert!(
        back.y + back.h <= screen.y + screen.h,
        "Back at {}..{} left the screen {}..{}",
        back.y,
        back.y + back.h,
        screen.y,
        screen.y + screen.h
    );
    assert_eq!(s.rects[3].h, 27, "the scroll inside absorbed the whole cut");
}

#[test]
fn a_fixed_size_panel_never_gives_height_back() {
    let (s, _) = solve_doc(
        r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame",
                "layout": { "w": { "grow": 1 }, "h": { "grow": 1 },
                            "align": "center", "justify": "center" },
                "children": [
                    { "type": "column", "id": "panel", "layout": { "w": 80, "h": 90 }, "children": [
                        { "type": "scroll", "id": "list", "layout": { "h": { "grow": 1 }, "min_h": 12 },
                          "children": [ { "type": "checkbox", "id": "a" } ] }
                    ] }
                ] }
        }"#,
        (80, 60),
    );
    assert_eq!(
        s.rects[1].h, 90,
        "an authored height is kept, and overflows"
    );
}

#[test]
fn scrollbar_reflows_wrapped_list_rows_and_updates_the_scroll_range() {
    let doc = Document::from_json(
        r#"{
        "format": 1, "kind": "test:wrapped_rows", "class": "screen",
        "root": { "type": "scroll", "id": "scroll", "layout": { "w": 60, "h": 15 },
            "children": [{ "type": "list", "bind": { "items": "rows" },
                "layout": { "align": "stretch" },
                "children": [{ "type": "column", "layout": { "align": "stretch" },
                    "children": [{ "type": "label", "wrap": true, "bind": { "text": "text" } }]
                }]
            }]
        }
    }"#,
    )
    .unwrap();
    let mut state = UiState::new();
    let row = [("text".into(), UiValue::Str("abcdefghij".into()))]
        .into_iter()
        .collect();
    state.set("rows", UiValue::List(std::sync::Arc::new(vec![row; 3])));
    let tree = InstTree::expand(&doc, &state);
    let solved = solve(&tree, &MockEnv, (100, 100), &|_| 0);
    let mut bottom = 0;
    for i in 0..tree.len() {
        let inst = tree.get(i as u32);
        if !matches!(inst.node.kind, NodeKind::Label { .. }) {
            continue;
        }
        let r = solved.rects[i];
        let required = MockEnv
            .leaf_size(inst.node, inst.text.as_deref(), None, Some(r.w))
            .1;
        assert!(r.h >= required, "wrapped ink must fit the row");
        assert!(r.y >= bottom, "rows must not overlap after reflow");
        bottom = r.y + r.h;
    }
    assert!(
        solved.scroll_content[0].unwrap().1 >= bottom - solved.rects[0].y,
        "last line remains reachable"
    );
}
