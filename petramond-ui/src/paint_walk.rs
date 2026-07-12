//! The paint pass: walk the solved instance tree in arena (paint) order and
//! emit every themed quad and glyph into the [`DrawList`].
//!
//! Face states resolve here from [`FrameState`] + bindings (hover, pressed,
//! focus, disabled, on/off, selected) — the same inputs interaction uses, so
//! what you see is what clicks. Host-drawn content (item icons, hearts) is
//! NOT painted here; the host layers it over this list using the frame's
//! named rects.

use crate::doc::{GaugeMode, NodeKind, ScrollAxis};
use crate::input::{FrameState, PreviewState};
use crate::layout::{grid_cell, RectI, SlotMetrics, Solved};
use crate::paint::{Painter, TexId};
use crate::theme::{Part, Theme};
use crate::tree::{InstTree, ROOT};
use crate::widget;

/// Resolves document-relative image names to host texture ids + pixel sizes.
pub trait DocImages {
    fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))>;
}

/// No document images (screens that use none; tests).
pub struct NoImages;

impl DocImages for NoImages {
    fn resolve(&self, _name: &str) -> Option<(u16, (u32, u32))> {
        None
    }
}

pub(crate) struct PaintCtx<'a> {
    pub tree: &'a InstTree<'a>,
    pub solved: &'a Solved,
    pub theme: &'a Theme,
    pub fs: &'a FrameState,
    pub images: &'a dyn DocImages,
    pub metrics: SlotMetrics,
    /// Topmost pointer-target instance under the cursor.
    pub hover: Option<u32>,
    /// Hovered slot cell as `(inst, cell)`.
    pub slot_hover: Option<(u32, u32)>,
    /// Hovered list row as `(list inst, row)`.
    pub row_hover: Option<(u32, u32)>,
    pub preview: Option<&'a PreviewState>,
}

impl PaintCtx<'_> {
    pub fn paint(&self, p: &mut Painter<'_>) {
        self.node(ROOT, None, p);
    }

    fn node(&self, i: u32, row_state: Option<&str>, p: &mut Painter<'_>) {
        let inst = self.tree.get(i);
        let rect = self.solved.rects[i as usize];
        let clip = self.solved.clips[i as usize];
        let part = self.theme.part_for(inst.node);
        let atlas = self.theme.atlas.size;
        let text_color = |color_key: &str| self.theme.color(color_key);
        let label_color = |part: Option<&Part>, enabled: bool| {
            if !enabled {
                text_color("text_disabled")
            } else {
                match part.and_then(|p| p.label_color.as_deref()) {
                    Some(key) => text_color(key),
                    None => text_color("text"),
                }
            }
        };

        let hovered = self.hover == Some(i)
            || self
                .preview
                .is_some_and(|pv| pv.hover.as_ref() == inst.key.as_ref() && inst.key.is_some());
        let pressed = (self
            .fs
            .active
            .as_ref()
            .is_some_and(|(k, _)| Some(k) == inst.key.as_ref())
            && hovered)
            // The frame a click fires keeps the pressed face: the host applies
            // the event (e.g. a list selection) only before the NEXT frame, so
            // without this bridge a selected row would flash unpressed for one
            // frame between release and the rebound selection.
            || (inst.key.is_some() && self.fs.clicked.as_ref() == inst.key.as_ref())
            || self
                .preview
                .is_some_and(|pv| pv.pressed.as_ref() == inst.key.as_ref() && inst.key.is_some());
        let focused = inst.key.is_some()
            && (self.fs.focus.as_ref() == inst.key.as_ref()
                || self
                    .preview
                    .is_some_and(|pv| pv.focus.as_ref() == inst.key.as_ref()));

        match &inst.node.kind {
            NodeKind::Frame
            | NodeKind::Row
            | NodeKind::Column
            | NodeKind::List
            | NodeKind::Scroll { .. } => {
                if let Some(part) = part {
                    let state = row_state.unwrap_or("default");
                    if let Some(face) = part.face(state) {
                        p.nine_slice(
                            TexId::ThemeAtlas,
                            rect,
                            face.rect,
                            face.slice.unwrap_or([0; 4]),
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                    }
                }
            }
            NodeKind::Spacer | NodeKind::Hook => {}
            NodeKind::Label { wrap, scale, .. } => {
                let text = inst.text.as_deref().unwrap_or("");
                let color = label_color(part, inst.enabled);
                if *scale > 1 {
                    p.text_scaled(text, rect.x, rect.y, *scale, color, clip);
                } else if *wrap {
                    p.text_wrapped(text, rect, color, clip);
                } else {
                    p.text_ellipsized(text, rect, color, clip);
                }
            }
            NodeKind::Image { fit, .. } => {
                if let Some((tex, size)) = inst.image_name().and_then(|n| self.images.resolve(n)) {
                    let src = [0, 0, size.0, size.1];
                    match fit {
                        crate::doc::ImageFit::Stretch => {
                            p.sprite(TexId::DocImage(tex), rect, src, size, [1.0; 4], clip)
                        }
                        crate::doc::ImageFit::Cover => {
                            p.cover_sprite(TexId::DocImage(tex), rect, src, size, [1.0; 4], clip)
                        }
                        crate::doc::ImageFit::Tile => {
                            p.tiled_sprite(TexId::DocImage(tex), rect, src, size, [1.0; 4], clip)
                        }
                        crate::doc::ImageFit::Slice(insets) => p.nine_slice(
                            TexId::DocImage(tex),
                            rect,
                            src,
                            *insets,
                            size,
                            [1.0; 4],
                            clip,
                        ),
                    }
                }
            }
            NodeKind::Rotimage { pivot, .. } => {
                if let Some((tex, size)) = inst.image_name().and_then(|n| self.images.resolve(n)) {
                    p.rotated_sprite(
                        TexId::DocImage(tex),
                        rect,
                        [0, 0, size.0, size.1],
                        size,
                        inst.value_f32.unwrap_or(0.0),
                        *pivot,
                        [1.0; 4],
                        clip,
                    );
                }
            }
            NodeKind::Button { icon, .. } => {
                let selected = row_state == Some("selected");
                let state = if !inst.enabled {
                    "disabled"
                } else if pressed || selected {
                    "pressed"
                } else if hovered {
                    "hover"
                } else {
                    "default"
                };
                let mut label_off = [0, 0];
                if let Some(part) = part {
                    if let Some(face) = part.face(state) {
                        p.nine_slice(
                            TexId::ThemeAtlas,
                            rect,
                            face.rect,
                            face.slice.unwrap_or([0; 4]),
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                    }
                    if state == "pressed" {
                        label_off = part.pressed_label_offset;
                    }
                }
                // Icon + label centred as one block (icon alone when no text).
                let icon_part = icon.as_deref().and_then(|k| self.theme.part(k));
                let (icon_w, icon_h) = icon_part.map(|p| p.natural()).unwrap_or((0, 0));
                let text = inst.text.as_deref().unwrap_or("");
                let tw = crate::text::width(text);
                let gap = if icon_w > 0 && tw > 0 { 4 } else { 0 };
                let block_w = icon_w + gap + tw;
                let mut cx = rect.x + (rect.w - block_w) / 2 + label_off[0];
                if let Some(face) = icon_part.and_then(|pt| pt.face("default")) {
                    p.sprite(
                        TexId::ThemeAtlas,
                        RectI {
                            x: cx,
                            y: rect.y + (rect.h - icon_h) / 2 + label_off[1],
                            w: icon_w,
                            h: icon_h,
                        },
                        face.rect,
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                    cx += icon_w + gap;
                }
                if !text.is_empty() {
                    p.text(
                        text,
                        cx,
                        rect.y + (rect.h - crate::text::GLYPH_H) / 2 + label_off[1],
                        label_color(part, inst.enabled),
                        clip,
                    );
                }
            }
            NodeKind::Checkbox | NodeKind::Toggle { .. } => {
                let state = if !inst.enabled {
                    "disabled"
                } else if inst.value_bool.unwrap_or(false) {
                    "on"
                } else {
                    "off"
                };
                if let Some(face) = part.and_then(|p| p.face(state)) {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        rect,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
                if let NodeKind::Toggle { icon: Some(icon) } = &inst.node.kind {
                    let icon_part = self.theme.part(icon);
                    let (icon_w, icon_h) = icon_part.map(|p| p.natural()).unwrap_or((0, 0));
                    if let Some(face) = icon_part.and_then(|pt| pt.face("default")) {
                        p.sprite(
                            TexId::ThemeAtlas,
                            RectI {
                                x: rect.x + (rect.w - icon_w) / 2,
                                y: rect.y + (rect.h - icon_h) / 2,
                                w: icon_w,
                                h: icon_h,
                            },
                            face.rect,
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                    }
                }
            }
            NodeKind::Slider { min, max, .. } => {
                let track_h = part.map(|p| p.natural().1).filter(|h| *h > 0).unwrap_or(6);
                let track = RectI {
                    x: rect.x,
                    y: rect.y + (rect.h - track_h) / 2,
                    w: rect.w,
                    h: track_h,
                };
                if let Some(face) = part.and_then(|p| p.face("default")) {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        track,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
                let dragging = matches!(
                    &self.fs.drag,
                    Some(crate::input::Drag::Slider { key }) if Some(key) == inst.key.as_ref()
                );
                let value = inst.value_f32.unwrap_or(*min);
                let handle = widget::slider_handle(rect, self.theme, *min, *max, value);
                let hstate = if !inst.enabled {
                    "disabled"
                } else if dragging {
                    "pressed"
                } else if hovered {
                    "hover"
                } else {
                    "default"
                };
                if let Some(face) = self
                    .theme
                    .part("slider.handle")
                    .and_then(|p| p.face(hstate))
                {
                    p.sprite(TexId::ThemeAtlas, handle, face.rect, atlas, [1.0; 4], clip);
                }
            }
            NodeKind::TextInput { placeholder, .. } => {
                let state = if !inst.enabled {
                    "disabled"
                } else if focused {
                    "focus"
                } else {
                    "default"
                };
                if let Some(face) = part.and_then(|p| p.face(state)) {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        rect,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
                let pad = self.theme.metrics.button_pad;
                let text_rect = widget::input_text_rect(rect, pad);
                let visible = widget::input_visible_chars(text_rect.w);
                let ty = rect.y + (rect.h - crate::text::GLYPH_H) / 2;
                let editor = inst.key.as_ref().and_then(|k| self.fs.editors.get(k));
                match editor {
                    Some(editor) => {
                        let view = editor.render(visible, focused, self.fs.now);
                        p.text_input_view(
                            &view,
                            text_rect.x,
                            ty,
                            text_color("text"),
                            self.theme.color("selection"),
                            clip,
                        );
                    }
                    None => {
                        let bound = inst.text.as_deref().unwrap_or("");
                        if bound.is_empty() {
                            if let Some(ph) = placeholder.as_deref() {
                                let shown: String = ph.chars().take(visible).collect();
                                p.text(&shown, text_rect.x, ty, text_color("text_muted"), clip);
                            }
                        } else {
                            let shown: String = bound.chars().take(visible).collect();
                            p.text(&shown, text_rect.x, ty, text_color("text"), clip);
                        }
                    }
                }
            }
            NodeKind::Slot { .. } | NodeKind::SlotGrid { .. } => {
                let cells = match inst.node.kind {
                    NodeKind::SlotGrid { cols, rows, .. } => cols * rows,
                    _ => 1,
                };
                let cols = match inst.node.kind {
                    NodeKind::SlotGrid { cols, .. } => cols,
                    _ => 1,
                };
                for c in 0..cells {
                    let cell = grid_cell(rect, cols, c, self.metrics);
                    if let Some(face) = part.and_then(|p| p.face("default")) {
                        p.nine_slice(
                            TexId::ThemeAtlas,
                            cell,
                            face.rect,
                            face.slice.unwrap_or([0; 4]),
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                    }
                    // Overlay faces: the bound `selected` cell (hotbar active
                    // slot) and the hovered cell.
                    let selected = inst.selected == Some(c as i32);
                    let overlay = if selected {
                        part.and_then(|p| p.face_if("selected").or_else(|| p.face_if("hover")))
                    } else if self.slot_hover == Some((i, c)) {
                        part.and_then(|p| p.face_if("hover"))
                    } else {
                        None
                    };
                    if let Some(face) = overlay {
                        p.nine_slice(
                            TexId::ThemeAtlas,
                            cell,
                            face.rect,
                            face.slice.unwrap_or([0; 4]),
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                    }
                }
            }
            NodeKind::Gauge { mode } => {
                let frac = inst.value_f32.unwrap_or(0.0).clamp(0.0, 1.0);
                if let Some(face) = part.and_then(|p| p.face("empty")) {
                    p.sprite(TexId::ThemeAtlas, rect, face.rect, atlas, [1.0; 4], clip);
                }
                if frac > 0.0 {
                    let fill = match mode {
                        GaugeMode::GrowLr => RectI {
                            x: rect.x,
                            y: rect.y,
                            w: (rect.w as f32 * frac).round() as i32,
                            h: rect.h,
                        },
                        GaugeMode::DepleteTd => {
                            let keep = (rect.h as f32 * frac).round() as i32;
                            RectI {
                                x: rect.x,
                                y: rect.y + rect.h - keep,
                                w: rect.w,
                                h: keep,
                            }
                        }
                    };
                    let fill_clip = match clip {
                        Some(c) => fill.intersect(c),
                        None => fill,
                    };
                    if let Some(face) = part.and_then(|p| p.face("full")) {
                        p.sprite(
                            TexId::ThemeAtlas,
                            rect,
                            face.rect,
                            atlas,
                            [1.0; 4],
                            Some(fill_clip),
                        );
                    }
                }
            }
            NodeKind::Badge { .. } => {
                if let Some(face) = part.and_then(|p| p.face("default")) {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        rect,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
                if let Some(text) = inst.text.as_deref() {
                    let tw = crate::text::width(text);
                    p.text(
                        text,
                        rect.x + (rect.w - tw) / 2,
                        rect.y + (rect.h - crate::text::GLYPH_H) / 2,
                        label_color(part, inst.enabled),
                        clip,
                    );
                }
            }
            NodeKind::Alert { .. } => {
                let insets = part
                    .and_then(|p| p.face("default"))
                    .and_then(|f| f.slice)
                    .unwrap_or([4, 4, 4, 4]);
                if let Some(face) = part.and_then(|p| p.face("default")) {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        rect,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
                let icon_key = format!(
                    "{}.icon",
                    inst.node.style.as_deref().unwrap_or_else(|| {
                        crate::theme::default_style_key(&inst.node.kind).unwrap_or("alert.info")
                    })
                );
                let mut tx = rect.x + insets[0];
                if let Some(icon) = self.theme.part(&icon_key) {
                    let (iw, ih) = icon.natural();
                    if let Some(face) = icon.face("default") {
                        p.sprite(
                            TexId::ThemeAtlas,
                            RectI {
                                x: tx,
                                y: rect.y + (rect.h - ih) / 2,
                                w: iw,
                                h: ih,
                            },
                            face.rect,
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                    }
                    tx += iw + 4;
                }
                if let Some(text) = inst.text.as_deref() {
                    // Wrap to the frame's interior; centre the wrapped block
                    // vertically (single lines land where they always did).
                    let text_w = (rect.x + rect.w - insets[2] - tx).max(crate::text::GLYPH_W);
                    let (_, block_h) = crate::text::measure(text, Some(text_w));
                    p.text_wrapped(
                        text,
                        RectI {
                            x: tx,
                            y: rect.y + (rect.h - block_h) / 2,
                            w: text_w,
                            h: block_h,
                        },
                        label_color(part, inst.enabled),
                        clip,
                    );
                }
            }
        }

        // Children in arena order; list stamps carry their row face state.
        let is_list = matches!(inst.node.kind, NodeKind::List);
        for (row, &c) in inst.children.iter().enumerate() {
            let child_row_state = if is_list {
                let child_enabled = self.tree.get(c).enabled;
                let selected = child_enabled && inst.selected == Some(row as i32);
                let hovered_row = child_enabled && self.row_hover == Some((i, row as u32));
                Some(if !child_enabled {
                    "disabled"
                } else if selected {
                    "selected"
                } else if hovered_row {
                    "hover"
                } else {
                    "default"
                })
            } else {
                None
            };
            self.node(c, child_row_state, p);
        }

        // Scrollbar chrome overlays the scroll node's children.
        if let NodeKind::Scroll {
            axis: ScrollAxis::Vertical,
        } = inst.node.kind
        {
            let content = self.solved.scroll_content[i as usize].unwrap_or((0, 0));
            let offset = inst
                .key
                .as_ref()
                .map(|k| self.fs.scroll_offset(k))
                .unwrap_or(0);
            let view = widget::scroll_view_rect(self.theme, inst.node, rect);
            if let Some((track, thumb)) = widget::scrollbar(
                view,
                rect.h,
                content.1,
                offset,
                self.theme.metrics.scrollbar_w,
            ) {
                if let Some(face) = self
                    .theme
                    .part("scrollbar.track")
                    .and_then(|p| p.face("default"))
                {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        track,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
                let dragging = matches!(
                    &self.fs.drag,
                    Some(crate::input::Drag::ScrollThumb { key, .. }) if Some(key) == inst.key.as_ref()
                );
                let tstate = if dragging { "hover" } else { "default" };
                if let Some(face) = self
                    .theme
                    .part("scrollbar.thumb")
                    .and_then(|p| p.face(tstate))
                {
                    p.nine_slice(
                        TexId::ThemeAtlas,
                        thumb,
                        face.rect,
                        face.slice.unwrap_or([0; 4]),
                        atlas,
                        [1.0; 4],
                        clip,
                    );
                }
            }
        }
    }
}
