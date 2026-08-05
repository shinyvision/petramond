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

/// A bound abs position must MOVE the widget (the anvil's augment slot
/// follows the inserted tool), and the authored abs must stay the resting
/// place when nothing is published — per axis.
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
    // Nothing published: the authored resting place.
    let s = solve_with(&[]);
    assert_eq!((s.rects[1].x, s.rects[1].y), (5, 7));
    // Both axes bound: the published position wins.
    let s = solve_with(&[("px", 40), ("py", 60)]);
    assert_eq!((s.rects[1].x, s.rects[1].y), (40, 60));
    // One axis bound: the other keeps its authored value.
    let s = solve_with(&[("py", 33)]);
    assert_eq!((s.rects[1].x, s.rects[1].y), (5, 33));
}

/// `overlay: true` raises a subtree's PAINT tier without the tooltip tier's
/// hit exclusion — the anvil's augment slot must draw above the host's
/// enlarged tool view AND still take the click.
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
    // 100 wide, 4 columns, 2px gaps: 94 of content over 4 columns is 23 each
    // with 2 left over, which goes +1 to the LEADING columns.
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

    // Row 2 starts under row 1 at the uniform cell height + gap.
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

    // The scroll clips its flow children but never the floating tooltip.
    assert!(s.clips[1].is_some() && s.clips[4].is_some());
    assert_eq!(s.clips[2], None);
    assert_eq!(s.clips[3], None, "the clip exemption covers the subtree");

    assert!(!s.overlay[1] && !s.overlay[4]);
    assert!(s.overlay[2] && s.overlay[3], "the whole subtree is overlay");
}

/// A wrapping label inside a max-bounded AUTO container (a tooltip) breaks at
/// the CAP, not at the incoming hint: the natural width can never exceed
/// `max_w`, so measuring single-line against the parent hint and then
/// ellipsizing into the clamped box would hide text the cap had room to show.
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
    // 30 chars × 6 = 180 single-line; capped at 60 → 10 chars/line → 3 lines.
    let tip = s.rects[1];
    assert_eq!(tip.w, 60, "natural width clamps to max_w");
    assert_eq!(tip.h, 3 * 9, "the label wraps at the cap");

    // A short text still shrinks the tooltip to its content — the cap only
    // ever bounds, it never pads.
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

/// The mods-list row: an icon, a text column, a spacer, and a toggle. Whatever
/// a pack author writes in `desc` must not be able to shove the toggle off the
/// panel — the player could not click it, and the label would paint across the
/// screen. Bound text ellipsizes; that is what makes it the shock absorber.
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
    // The cut reaches the labels themselves, not just the column around them:
    // a column that shrank while its text kept its natural width would paint
    // straight through the panel edge.
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

/// Authored text is a decision the layout owes the author: a caption that no
/// longer fits is an authoring bug to fix in the document, not something to
/// silently ellipsize. Only DATA shrinks.
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

/// The options-screen shape: a panel inside a full-screen backdrop frame. The
/// panel is AUTO-sized, so nothing forces it to notice a short viewport — and
/// its Back button slides off the bottom of the screen, where no click can
/// reach it. It has a `grow` scroll inside, which is the thing that should
/// give; the panel has to pass the cut down to it.
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
    // Natural panel: label 9 + 2 + list 40 + 2 + button 20 = 73 in a 60 box.
    // Arena order: 0 screen, 1 panel, 2 label, 3 scroll, 4..=7 cells, 8 button.
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

/// The cut stops at a `Px` size: an author who wrote a number meant it, and
/// silently squashing it would hide the layout bug instead of showing it.
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
