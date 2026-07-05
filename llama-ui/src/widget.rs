//! Shared widget geometry: the same math feeds hit-testing (interact) and
//! drawing (paint), so a thumb can never render where it doesn't grab.
//!
//! Everything here is logical px and pure.

use crate::doc::{NodeKind, ScrollAxis};
use crate::layout::RectI;
use crate::theme::Theme;
use crate::tree::Inst;

/// Whether a pointer press can target this instance directly (used by the
/// topmost-hit scan). Containers are excluded — except list template stamps,
/// which are handled by their `list` parent.
pub(crate) fn pointer_target(inst: &Inst<'_>) -> bool {
    matches!(
        inst.node.kind,
        NodeKind::Button { .. }
            | NodeKind::Checkbox
            | NodeKind::Toggle
            | NodeKind::Slider { .. }
            | NodeKind::TextInput { .. }
            | NodeKind::Slot { .. }
            | NodeKind::SlotGrid { .. }
    )
}

/// Float point-in-rect over a logical rect (half-open, like `RectI`).
pub(crate) fn contains_f(r: RectI, x: f32, y: f32) -> bool {
    x >= r.x as f32 && x < (r.x + r.w) as f32 && y >= r.y as f32 && y < (r.y + r.h) as f32
}

/// The vertical scrollbar geometry of a scroll node, or `None` when the
/// content fits (no bar). `view` is the CHROME rect the bar may occupy (the
/// node rect inset by its styled border, so the bar never covers a frame);
/// `viewport_len` is the node's full scroll-axis length — offsets stay in
/// node space.
pub(crate) fn scrollbar(
    view: RectI,
    viewport_len: i32,
    content_h: i32,
    offset: i32,
    bar_w: i32,
) -> Option<(RectI, RectI)> {
    if content_h <= viewport_len || view.h <= 0 {
        return None;
    }
    let track = RectI {
        x: view.x + view.w - bar_w,
        y: view.y,
        w: bar_w,
        h: view.h,
    };
    let thumb_h = ((view.h * viewport_len) / content_h).clamp(8.min(view.h), view.h);
    let range = view.h - thumb_h;
    let max_off = content_h - viewport_len;
    let thumb_y = if max_off > 0 {
        track.y + (offset.clamp(0, max_off) * range) / max_off
    } else {
        track.y
    };
    let thumb = RectI {
        x: track.x,
        y: thumb_y,
        w: bar_w,
        h: thumb_h,
    };
    Some((track, thumb))
}

/// Map a thumb-drag pointer y (logical, minus grab offset) back to a scroll
/// offset.
pub(crate) fn scroll_offset_for_thumb_y(
    view: RectI,
    viewport_len: i32,
    content_h: i32,
    thumb_top: f32,
) -> i32 {
    let thumb_h = ((view.h * viewport_len) / content_h.max(1)).clamp(8.min(view.h), view.h);
    let range = (view.h - thumb_h).max(1);
    let max_off = (content_h - viewport_len).max(0);
    let frac = ((thumb_top - view.y as f32) / range as f32).clamp(0.0, 1.0);
    (frac * max_off as f32).round() as i32
}

/// The chrome rect scrollbars occupy: the node rect inset by its styled
/// border so the bar sits inside a framed scroll region.
pub(crate) fn scroll_view_rect(
    theme: &Theme,
    node: &crate::doc::Node,
    rect: RectI,
) -> RectI {
    rect.inset(theme.container_insets(node))
}

/// Clamp a scroll offset against solved content.
pub(crate) fn clamp_scroll(offset: i32, viewport: i32, content: i32) -> i32 {
    offset.clamp(0, (content - viewport).max(0))
}

/// The scroll axis length of a scroll node's viewport/content pair.
pub(crate) fn scroll_lengths(
    axis: ScrollAxis,
    rect: RectI,
    content: (i32, i32),
) -> (i32, i32) {
    match axis {
        ScrollAxis::Vertical => (rect.h, content.1),
        ScrollAxis::Horizontal => (rect.w, content.0),
    }
}

/// Slider handle rect for a value in `min..=max` over the track `rect`.
pub(crate) fn slider_handle(rect: RectI, theme: &Theme, min: f32, max: f32, value: f32) -> RectI {
    let (hw, hh) = theme
        .part("slider.handle")
        .map(|p| p.natural())
        .unwrap_or((6, rect.h + 4));
    let span = (rect.w - hw).max(0);
    let frac = if max > min {
        ((value - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    RectI {
        x: rect.x + (frac * span as f32).round() as i32,
        y: rect.y + (rect.h - hh) / 2,
        w: hw,
        h: hh,
    }
}

/// Slider value for a pointer x over the track `rect`, quantized to `step`.
pub(crate) fn slider_value_at(
    rect: RectI,
    x: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
) -> f32 {
    let frac = ((x - rect.x as f32) / rect.w.max(1) as f32).clamp(0.0, 1.0);
    let mut v = min + frac * (max - min);
    if let Some(step) = step.filter(|s| *s > 0.0) {
        v = min + ((v - min) / step).round() * step;
    }
    v.clamp(min, max)
}

/// How many characters fit in a text input's inner width.
pub(crate) fn input_visible_chars(inner_w: i32) -> usize {
    if inner_w < crate::text::GLYPH_W {
        0
    } else {
        ((inner_w + 1) / crate::text::ADVANCE) as usize
    }
}

/// The text interior of an input rect (theme-metric horizontal inset).
pub(crate) fn input_text_rect(rect: RectI, pad: i32) -> RectI {
    RectI {
        x: rect.x + pad,
        y: rect.y,
        w: (rect.w - pad * 2).max(0),
        h: rect.h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_absent_when_content_fits() {
        let r = RectI { x: 0, y: 0, w: 50, h: 40 };
        assert!(scrollbar(r, r.h, 40, 0, 8).is_none());
        assert!(scrollbar(r, r.h, 30, 0, 8).is_none());
    }

    #[test]
    fn thumb_maps_offset_round_trip() {
        let r = RectI { x: 0, y: 0, w: 50, h: 40 };
        let content = 120;
        for off in [0, 13, 40, 80] {
            let (_, thumb) = scrollbar(r, r.h, content, off, 8).unwrap();
            let back = scroll_offset_for_thumb_y(r, r.h, content, thumb.y as f32);
            assert!((back - off).abs() <= 2, "offset {off} → thumb → {back}");
        }
        // Extremes map exactly.
        let (_, top) = scrollbar(r, r.h, content, 0, 8).unwrap();
        assert_eq!(top.y, 0);
        let (_, bottom) = scrollbar(r, r.h, content, 80, 8).unwrap();
        assert_eq!(bottom.y + bottom.h, 40);
    }

    #[test]
    fn framed_scroll_keeps_the_bar_inside_its_border() {
        // A 3px-bordered view: the track hugs the view's inner right edge
        // while offsets still span the node-space viewport/content.
        let node = RectI { x: 0, y: 0, w: 50, h: 40 };
        let view = node.inset([3, 3, 3, 3]);
        let (track, thumb) = scrollbar(view, node.h, 120, 0, 8).unwrap();
        assert_eq!(track.x + track.w, node.x + node.w - 3);
        assert_eq!(track.y, 3);
        assert_eq!(track.h, 34);
        assert_eq!(thumb.y, 3);
        let (_, bottom) = scrollbar(view, node.h, 120, 120 - node.h, 8).unwrap();
        assert_eq!(bottom.y + bottom.h, 3 + 34, "thumb ends at the view bottom");
    }

    #[test]
    fn slider_value_quantizes_and_clamps() {
        let r = RectI { x: 10, y: 0, w: 100, h: 6 };
        assert_eq!(slider_value_at(r, 10.0, 0.0, 1.0, None), 0.0);
        assert_eq!(slider_value_at(r, 110.0, 0.0, 1.0, None), 1.0);
        assert_eq!(slider_value_at(r, 300.0, 0.0, 1.0, None), 1.0);
        let v = slider_value_at(r, 62.0, 0.0, 100.0, Some(25.0));
        assert_eq!(v, 50.0, "52% snaps to the 50 step");
    }

    #[test]
    fn input_char_capacity_matches_font_advance() {
        assert_eq!(input_visible_chars(0), 0);
        assert_eq!(input_visible_chars(crate::text::GLYPH_W), 1);
        // 6 chars: 6*6-1 = 35 px.
        assert_eq!(input_visible_chars(35), 6);
        assert_eq!(input_visible_chars(34), 5);
    }
}
