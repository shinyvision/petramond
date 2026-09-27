//! The layout solver's window into a theme: widget natural sizes and the
//! metrics the solver reads.

use super::{default_style_key, FaceState, Theme};
use crate::doc::{Node, NodeKind};
use crate::layout::{LayoutEnv, SlotMetrics};

/// The logical size of a wrapped label capped at `max_lines` — the same
/// breaks [`crate::Painter::text_wrapped_lines`] draws.
fn wrapped_lines_size(
    font: &crate::text::Font,
    text: &str,
    avail_w: i32,
    small: bool,
    max_lines: u32,
    gui_scale: i32,
) -> (i32, i32) {
    let (step, full) = if small {
        ((gui_scale - 1).max(1), gui_scale.max(1))
    } else {
        (1, 1)
    };
    let down = |v: i32| (v * step + full - 1) / full;
    let lines = font.wrap(text, avail_w * full / step);
    let kept = lines.len().min(max_lines.max(1) as usize);
    let cut = kept < lines.len();
    let w = lines
        .iter()
        .take(kept)
        .map(|r| font.width(&text[r.clone()]))
        .max()
        .unwrap_or(0);
    let h = font.line_h() + (kept as i32 - 1).max(0) * font.line_advance();
    // A cut label's last line ellipsizes into the whole available width.
    let w = if cut { avail_w } else { down(w) };
    (w, down(h))
}

/// The solver's window into the theme + the host's document-image registry
/// (image natural sizes live outside the theme).
pub struct ThemeEnv<'a> {
    pub theme: &'a Theme,
    /// The host's integer GUI scale — only `small` labels read it.
    pub gui_scale: i32,
    pub image_size: &'a dyn Fn(&str) -> Option<(i32, i32)>,
}

impl LayoutEnv for ThemeEnv<'_> {
    fn gui_scale(&self) -> i32 {
        self.gui_scale.max(1)
    }

    fn leaf_size(
        &self,
        node: &Node,
        text: Option<&str>,
        image: Option<&str>,
        avail_w: Option<i32>,
    ) -> (i32, i32) {
        let m = &self.theme.metrics;
        let part_natural = |fallback: (i32, i32)| {
            self.theme
                .part_for(node)
                .map(|p| p.natural())
                .filter(|&(w, h)| w > 0 && h > 0)
                .unwrap_or(fallback)
        };
        match &node.kind {
            NodeKind::Label {
                wrap,
                scale,
                small,
                max_lines,
                ..
            } => {
                let text = text.unwrap_or("");
                let font = self.theme.ui_font();
                if let (true, Some(max_lines), Some(avail_w)) = (*wrap, *max_lines, avail_w) {
                    return wrapped_lines_size(
                        font,
                        text,
                        avail_w,
                        *small,
                        max_lines,
                        self.gui_scale(),
                    );
                }
                if *scale > 1 {
                    let (w, h) = font.measure(text, None);
                    (w * *scale as i32, h * *scale as i32)
                } else if *small {
                    // Drawn at `gui_scale - 1` physical px per font pixel, so
                    // it occupies that fraction of the logical box. Round UP:
                    // the reserved box must never be narrower than the ink.
                    let step = (self.gui_scale() - 1).max(1);
                    let full = self.gui_scale().max(1);
                    let down = |v: i32| (v * step + full - 1) / full;
                    let avail = avail_w.map(|w| w * self.gui_scale().max(1) / step);
                    let (w, h) = font.measure(text, if *wrap { avail } else { None });
                    (down(w), down(h))
                } else {
                    font.measure(text, if *wrap { avail_w } else { None })
                }
            }
            NodeKind::Button { icon, frames, .. } => {
                // An image-backed button's natural size is ONE frame of its
                // sheet, not the theme face's label metrics.
                if let Some(name) = image {
                    let sheet = (self.image_size)(name).unwrap_or((0, 0));
                    return crate::paint_walk::frame_cell(sheet, *frames);
                }
                let icon_w = icon
                    .as_deref()
                    .and_then(|k| {
                        self.theme
                            .part(k)
                            .map(|p| p.natural().0)
                            .or_else(|| (self.image_size)(k).map(|(w, _)| w))
                    })
                    .unwrap_or(0);
                let text_w = self.theme.ui_font().width(text.unwrap_or(""));
                let gap = if icon_w > 0 && text_w > 0 { 4 } else { 0 };
                (icon_w + gap + text_w + m.button_pad * 2, m.button_h)
            }
            NodeKind::Checkbox => part_natural((10, 10)),
            NodeKind::Toggle { .. } => part_natural((18, 10)),
            NodeKind::Slider { .. } => {
                let handle_h = self
                    .theme
                    .part("slider.handle")
                    .map(|p| p.natural().1)
                    .unwrap_or(0);
                let track_h = part_natural((m.slider_w, 6)).1;
                (m.slider_w, handle_h.max(track_h))
            }
            NodeKind::TextInput { .. } => (m.input_w, part_natural((m.input_w, m.button_h)).1),
            NodeKind::Slot { .. } => (m.slot, m.slot),
            NodeKind::SlotGrid { cols, rows, .. } => {
                let (c, r) = (*cols as i32, *rows as i32);
                (
                    c * m.slot + (c - 1).max(0) * m.slot_gap,
                    r * m.slot + (r - 1).max(0) * m.slot_gap,
                )
            }
            NodeKind::Gauge { .. } => part_natural((0, 0)),
            NodeKind::Image { frames, .. } => {
                let sheet = image
                    .and_then(|name| (self.image_size)(name))
                    .unwrap_or((0, 0));
                crate::paint_walk::frame_cell(sheet, *frames)
            }
            NodeKind::Rotimage { .. } => image
                .and_then(|name| (self.image_size)(name))
                .unwrap_or((0, 0)),
            NodeKind::Badge { .. } => {
                let text_w = self.theme.ui_font().width(text.unwrap_or(""));
                let h = part_natural((0, self.theme.ui_font().line_h() + m.badge_pad * 2)).1;
                (text_w + m.badge_pad * 2, h)
            }
            NodeKind::TabBar { tabs } => {
                let widths = crate::widget::tab_widths(self.theme, tabs);
                let gaps = m.tab_gap * (widths.len() as i32 - 1).max(0);
                (widths.iter().sum::<i32>() + gaps, m.tab_h)
            }
            NodeKind::Alert { .. } => {
                // Icon cell + text inside the frame insets; the text wraps
                // whenever the available width constrains it (an alert that
                // overflows its own frame is never right).
                let insets = self
                    .theme
                    .part_for(node)
                    .and_then(|p| p.face(FaceState::Default))
                    .and_then(|f| f.slice)
                    .unwrap_or([4, 4, 4, 4]);
                let icon = self
                    .theme
                    .part(&format!(
                        "{}.icon",
                        node.style.as_deref().unwrap_or_else(|| {
                            default_style_key(&node.kind).unwrap_or("alert.info")
                        })
                    ))
                    .map(|p| p.natural())
                    .unwrap_or((0, 0));
                let gap = if icon.0 > 0 { 4 } else { 0 };
                let chrome_w = insets[0] + icon.0 + gap + insets[2];
                let font = self.theme.ui_font();
                let text_avail = avail_w.map(|a| (a - chrome_w).max(font.max_advance()));
                let (text_w, text_h) = font.measure(text.unwrap_or(""), text_avail);
                (
                    chrome_w + text_w,
                    insets[1] + icon.1.max(text_h) + insets[3],
                )
            }
            _ => (0, 0),
        }
    }

    fn slot_metrics(&self) -> SlotMetrics {
        SlotMetrics {
            slot: self.theme.metrics.slot,
            gap: self.theme.metrics.slot_gap,
        }
    }

    fn container_insets(&self, node: &Node) -> [i32; 4] {
        self.theme.container_insets(node)
    }

    fn scrollbar_width(&self) -> i32 {
        self.theme.metrics.scrollbar_w
    }
}
