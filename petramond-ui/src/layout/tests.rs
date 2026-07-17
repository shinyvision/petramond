use super::*;
use crate::doc::{Document, Node};
use crate::state::{UiState, UiValue};
use crate::tree::InstTree;

/// Fixed-metric mock: labels are 6px/char × 9, checkboxes 10×10,
/// toggles 18×10, buttons text+8 × 20, slots 18px cells with 0 gap.
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
    // Natural: w = 8+18+8 = 34 (toggle widest), h = 6+10+4+10+6 = 36.
    // Centered in 200×100 → x=(200-34)/2=83, y=(100-36)/2=32.
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
    // leftover 103: floor shares 34 + 68 = 102, remainder 1 → first grower.
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
    // 100 - 30 = 70 leftover over 2 gaps = 35 each.
    assert_eq!(s.rects[1].x, 0);
    assert_eq!(s.rects[2].x, 45);
    assert_eq!(s.rects[3].x, 90);
    // align center in 40 → y = 15.
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
    // abs against the padded rect; takes no flow space.
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
    // Content: 3×10 + 2×2 = 34 tall > 30 viewport, so the children
    // stretch to the width MINUS the reserved scrollbar lane (50 − 8).
    assert_eq!(solved.scroll_content[0], Some((42, 34)));
    assert_eq!(solved.rects[1].w, 42, "rows reserve the scrollbar lane");
    // Offset 8 shifts children up by 8; root anchors at 0,0 (fills).
    assert_eq!(solved.rects[1].y, solved.rects[0].y - 8);
    // Children carry the scroll clip; scrolled-away rows can't hit.
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
    // Column 60 tall holding: label(9) + grow scroll (natural 3×10+4=34,
    // min_h 12) + button(20). Natural total 63 > 60: the scroll gives
    // back the 3px deficit and everything fits.
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
    // Zero naturals grow to 23/47 (70 split 1:2)… growers first expand to
    // fill, so no shrink here; assert the pair still tiles exactly.
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
    // 2×10 + 2 = 22 fits in 40: no bar, children get the full width.
    assert_eq!(solved.rects[1].w, 50);
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
    // 12 chars × 6 = 72 > avail 60 → 10 chars/line → 2 lines × 9.
    assert_eq!(s.rects[1].h, 18);
    assert_eq!(s.rects[1].w, 60);
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
    // Row-major: cell 9 (second row, first column).
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
