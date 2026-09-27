use crate::doc::{ScrollAxis, TabSpec};
use crate::layout::RectI;
use crate::text::Font;
use crate::theme::{FaceState, Theme};
use crate::tree::Inst;

pub(crate) fn pointer_target(inst: &Inst<'_>) -> bool {
    crate::widget_policy::pointer_target(&inst.node.kind)
}

pub(crate) fn tab_widths(theme: &Theme, tabs: &[TabSpec]) -> Vec<i32> {
    let pad = theme.metrics.button_pad;
    tabs.iter()
        .map(|tab| {
            let icon_w = tab
                .icon
                .as_deref()
                .and_then(|k| theme.part(k))
                .map(|p| p.natural().0)
                .unwrap_or(0);
            let text_w = theme.ui_font().width(tab.label.as_deref().unwrap_or(""));
            let gap = if icon_w > 0 && text_w > 0 { 4 } else { 0 };
            icon_w + gap + text_w + pad * 2
        })
        .collect()
}

pub(crate) fn tab_cell(rect: RectI, widths: &[i32], gap: i32, i: usize) -> RectI {
    let x = rect.x + widths[..i].iter().map(|w| w + gap).sum::<i32>();
    RectI {
        x,
        y: rect.y,
        w: widths[i],
        h: rect.h,
    }
}

pub(crate) fn tab_hit(rect: RectI, widths: &[i32], gap: i32, x: f32, y: f32) -> Option<u32> {
    (0..widths.len())
        .find_map(|i| contains_f(tab_cell(rect, widths, gap, i), x, y).then_some(i as u32))
}

pub(crate) fn contains_f(r: RectI, x: f32, y: f32) -> bool {
    x >= r.x as f32 && x < (r.x + r.w) as f32 && y >= r.y as f32 && y < (r.y + r.h) as f32
}

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

pub(crate) fn scroll_view_rect(theme: &Theme, node: &crate::doc::Node, rect: RectI) -> RectI {
    rect.inset(theme.container_insets(node))
}

pub(crate) fn clamp_scroll(offset: i32, viewport: i32, content: i32) -> i32 {
    offset.clamp(0, (content - viewport).max(0))
}

pub(crate) fn scroll_lengths(axis: ScrollAxis, rect: RectI, content: (i32, i32)) -> (i32, i32) {
    match axis {
        ScrollAxis::Vertical => (rect.h, content.1),
        ScrollAxis::Horizontal => (rect.w, content.0),
    }
}

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

pub(crate) fn slider_value_at(rect: RectI, x: f32, min: f32, max: f32, step: Option<f32>) -> f32 {
    let frac = ((x - rect.x as f32) / rect.w.max(1) as f32).clamp(0.0, 1.0);
    let mut v = min + frac * (max - min);
    if let Some(step) = step.filter(|s| *s > 0.0) {
        v = min + ((v - min) / step).round() * step;
    }
    v.clamp(min, max)
}

pub(crate) fn button_face_state(
    enabled: bool,
    selected: bool,
    pressed: bool,
    hovered: bool,
    has_selected_face: bool,
) -> FaceState {
    if !enabled {
        FaceState::Disabled
    } else if selected && has_selected_face {
        FaceState::Selected
    } else if pressed || selected {
        FaceState::Pressed
    } else if hovered {
        FaceState::Hover
    } else {
        FaceState::Default
    }
}

pub(crate) fn toggle_face_chain(
    enabled: bool,
    on: bool,
    pressed: bool,
    hovered: bool,
) -> [FaceState; 3] {
    let base = if on { FaceState::On } else { FaceState::Off };
    if !enabled {
        [FaceState::Disabled; 3]
    } else if pressed {
        [
            if on {
                FaceState::OnPressed
            } else {
                FaceState::OffPressed
            },
            FaceState::Pressed,
            base,
        ]
    } else if hovered {
        [
            if on {
                FaceState::OnHover
            } else {
                FaceState::OffHover
            },
            FaceState::Hover,
            base,
        ]
    } else {
        [base; 3]
    }
}

pub(crate) fn input_visible_chars(font: &Font, inner_w: i32) -> usize {
    if inner_w <= 0 {
        return 0;
    }
    (inner_w / font.max_advance().max(1)).max(1) as usize
}

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
    fn an_authored_selected_face_wins_over_the_pressed_fallback() {
        let with =
            |selected, pressed, hovered| button_face_state(true, selected, pressed, hovered, true);
        let without =
            |selected, pressed, hovered| button_face_state(true, selected, pressed, hovered, false);
        assert_eq!(with(true, false, false), FaceState::Selected);
        assert_eq!(without(true, false, false), FaceState::Pressed);
        assert_eq!(with(false, true, false), FaceState::Pressed);
        assert_eq!(with(false, false, true), FaceState::Hover);
        assert_eq!(with(false, false, false), FaceState::Default);
        assert_eq!(
            button_face_state(false, true, false, false, true),
            FaceState::Disabled
        );
    }

    #[test]
    fn the_toggle_chain_always_ends_on_a_plain_state_name() {
        for (enabled, on, pressed, hovered) in [
            (true, false, false, false),
            (true, true, true, true),
            (true, false, false, true),
            (false, true, true, true),
        ] {
            let chain = toggle_face_chain(enabled, on, pressed, hovered);
            let expect = if !enabled {
                FaceState::Disabled
            } else if on {
                FaceState::On
            } else {
                FaceState::Off
            };
            assert_eq!(chain[2], expect, "{enabled} {on} {pressed} {hovered}");
        }
    }

    #[test]
    fn scrollbar_absent_when_content_fits() {
        let r = RectI {
            x: 0,
            y: 0,
            w: 50,
            h: 40,
        };
        assert!(scrollbar(r, r.h, 40, 0, 8).is_none());
        assert!(scrollbar(r, r.h, 30, 0, 8).is_none());
    }

    #[test]
    fn thumb_maps_offset_round_trip() {
        let r = RectI {
            x: 0,
            y: 0,
            w: 50,
            h: 40,
        };
        let content = 120;
        for off in [0, 13, 40, 80] {
            let (_, thumb) = scrollbar(r, r.h, content, off, 8).unwrap();
            let back = scroll_offset_for_thumb_y(r, r.h, content, thumb.y as f32);
            assert!((back - off).abs() <= 2, "offset {off} → thumb → {back}");
        }
        let (_, top) = scrollbar(r, r.h, content, 0, 8).unwrap();
        assert_eq!(top.y, 0);
        let (_, bottom) = scrollbar(r, r.h, content, 80, 8).unwrap();
        assert_eq!(bottom.y + bottom.h, 40);
    }

    #[test]
    fn framed_scroll_keeps_the_bar_inside_its_border() {
        let node = RectI {
            x: 0,
            y: 0,
            w: 50,
            h: 40,
        };
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
        let r = RectI {
            x: 10,
            y: 0,
            w: 100,
            h: 6,
        };
        assert_eq!(slider_value_at(r, 10.0, 0.0, 1.0, None), 0.0);
        assert_eq!(slider_value_at(r, 110.0, 0.0, 1.0, None), 1.0);
        assert_eq!(slider_value_at(r, 300.0, 0.0, 1.0, None), 1.0);
        let v = slider_value_at(r, 62.0, 0.0, 100.0, Some(25.0));
        assert_eq!(v, 50.0, "52% snaps to the 50 step");
    }

    #[test]
    fn input_char_capacity_matches_font_advance() {
        let font = Font::builtin();
        assert_eq!(input_visible_chars(&font, 0), 0);
        assert_eq!(
            input_visible_chars(&font, 1),
            1,
            "a sliver still shows the caret glyph"
        );
        let six = font.max_advance() * 6;
        assert_eq!(input_visible_chars(&font, six), 6);
        assert_eq!(input_visible_chars(&font, six - 1), 5);
    }
}
