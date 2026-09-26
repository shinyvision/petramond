//! One painter per widget kind. Each draws only the instance itself; the
//! walk in the parent module paints children and scrollbar chrome after it.

use super::{frame_src, Here, PaintCtx};
use crate::doc::{GaugeMode, ImageFit, TabSpec};
use crate::layout::{grid_cell, RectI};
use crate::paint::{Fit, PaintStyle, Painter, SpriteSrc, TexId};
use crate::theme::{palette, FaceState, Part};
use crate::widget;

/// Gap between an icon and the label beside it, logical px.
const ICON_GAP: i32 = 4;

impl<'a> PaintCtx<'a> {
    /// A layout container: its part's face (a list stamp's row face) as
    /// chrome behind the children.
    pub(super) fn container(&self, n: &Here<'a>, p: &mut Painter<'_>) {
        let state = n.row.unwrap_or(FaceState::Default);
        if let Some(face) = n.part.and_then(|part| part.face(state)) {
            self.draw_face(p, face, n.rect, n.clip);
        }
    }

    pub(super) fn label(
        &self,
        n: &Here<'a>,
        wrap: bool,
        scale: u32,
        small: bool,
        p: &mut Painter<'_>,
    ) {
        let inst = n.inst;
        let text = inst.text.as_deref().unwrap_or("");
        // A bound palette entry outranks the style — the colour IS state —
        // but never the disabled face.
        let mut color = match inst.palette.as_deref() {
            Some(key) if inst.enabled => self.theme.color(key),
            _ => self.label_color(n.part, inst.enabled),
        };
        color[3] *= inst.text_opacity;
        let (rect, clip) = (n.rect, n.clip);
        if scale > 1 {
            p.text_scaled(text, rect.x, rect.y, scale, color, clip);
        } else if small && wrap {
            p.text_wrapped_small(text, rect, color, clip);
        } else if small {
            p.text_ellipsized_small(text, rect, color, clip);
        } else if wrap {
            p.text_wrapped(text, rect, color, clip);
        } else {
            p.text_ellipsized(text, rect, color, clip);
        }
    }

    pub(super) fn image(
        &self,
        n: &Here<'a>,
        fit: ImageFit,
        frames: Option<[u32; 2]>,
        fps: Option<f32>,
        p: &mut Painter<'_>,
    ) {
        let Some((tex, size)) = self.doc_image(n.inst.image_name()) else {
            return;
        };
        let src = SpriteSrc {
            tex: TexId::DocImage(tex),
            rect: frame_src(size, frames, n.inst.frame, fps, self.fs.now),
            tex_size: size,
        };
        let fit = match fit {
            ImageFit::Stretch => Fit::Stretch,
            ImageFit::Cover => Fit::Cover,
            ImageFit::Tile => Fit::Tile,
            ImageFit::Slice(insets) => Fit::NineSlice(insets),
        };
        p.sprite(&src, n.rect, fit, PaintStyle::plain(n.clip));
    }

    pub(super) fn rotimage(&self, n: &Here<'a>, pivot: Option<[f32; 2]>, p: &mut Painter<'_>) {
        let Some((tex, size)) = self.doc_image(n.inst.image_name()) else {
            return;
        };
        let src = SpriteSrc {
            tex: TexId::DocImage(tex),
            rect: [0, 0, size.0, size.1],
            tex_size: size,
        };
        let fit = Fit::Rotated {
            angle: n.inst.value_f32.unwrap_or(0.0),
            pivot,
        };
        p.sprite(&src, n.rect, fit, PaintStyle::plain(n.clip));
    }

    pub(super) fn button(
        &self,
        n: &Here<'a>,
        icon: Option<&str>,
        frames: Option<[u32; 2]>,
        fps: Option<f32>,
        p: &mut Painter<'_>,
    ) {
        let inst = n.inst;
        let (rect, clip) = (n.rect, n.clip);
        let selected = n.row == Some(FaceState::Selected);
        // An image-backed button draws its document image instead of the
        // theme face; state affordance is a plain multiply tint.
        if let Some((tex, size)) = self.doc_image(inst.image_name()) {
            let src = SpriteSrc {
                tex: TexId::DocImage(tex),
                rect: frame_src(size, frames, inst.frame, fps, self.fs.now),
                tex_size: size,
            };
            let color = if !inst.enabled {
                [0.45, 0.45, 0.45, 1.0]
            } else if n.pressed {
                [0.7, 0.7, 0.7, 1.0]
            } else if n.hovered || selected {
                [1.15, 1.15, 1.15, 1.0]
            } else {
                [1.0; 4]
            };
            p.sprite(&src, rect, Fit::Stretch, PaintStyle { color, clip });
            return;
        }
        let state = widget::button_face_state(
            inst.enabled,
            selected,
            n.pressed,
            n.hovered,
            n.part
                .is_some_and(|p| p.face_if(FaceState::Selected).is_some()),
        );
        let mut label_off = [0, 0];
        if let Some(part) = n.part {
            if let Some(face) = part.face(state) {
                self.draw_face(p, face, rect, clip);
            }
            if state == FaceState::Pressed {
                label_off = part.pressed_label_offset;
            }
        }
        let text = inst.text.as_deref().unwrap_or("");
        let shifted = RectI {
            x: rect.x + label_off[0],
            y: rect.y + label_off[1],
            ..rect
        };
        let text_x = self.icon_and_label_block(p, shifted, icon, text, clip);
        if !text.is_empty() {
            // Centred while the block fits; once it does not, the run starts
            // at the padding edge and ellipsizes into the face instead of
            // painting out of the button.
            let pad = self.theme.metrics.button_pad;
            let x = text_x.max(rect.x + pad);
            let line_h = self.theme.ui_font().line_h();
            let line = RectI {
                x,
                y: rect.y + (rect.h - line_h) / 2 + label_off[1],
                w: (rect.x + rect.w - pad - x).max(0),
                h: line_h,
            };
            p.text_ellipsized(text, line, self.label_color(n.part, inst.enabled), clip);
        }
    }

    /// Draw `icon` (a part key) of an icon + label block centred in `cell`,
    /// returning where the label starts. Buttons and tabs share it.
    fn icon_and_label_block(
        &self,
        p: &mut Painter<'_>,
        cell: RectI,
        icon: Option<&str>,
        text: &str,
        clip: Option<RectI>,
    ) -> i32 {
        let icon_part = icon.and_then(|k| self.theme.part(k));
        let (icon_w, icon_h) = icon_part.map(Part::natural).unwrap_or((0, 0));
        let text_w = self.theme.ui_font().width(text);
        let gap = if icon_w > 0 && text_w > 0 {
            ICON_GAP
        } else {
            0
        };
        let mut x = cell.x + (cell.w - (icon_w + gap + text_w)) / 2;
        if let Some(face) = icon_part.and_then(|part| part.face(FaceState::Default)) {
            let at = RectI {
                x,
                y: cell.y + (cell.h - icon_h) / 2,
                w: icon_w,
                h: icon_h,
            };
            self.draw_sprite(p, face, at, PaintStyle::plain(clip));
            x += icon_w + gap;
        }
        x
    }

    pub(super) fn toggle(&self, n: &Here<'a>, p: &mut Painter<'_>) {
        let inst = n.inst;
        let chain = widget::toggle_face_chain(
            inst.enabled,
            inst.value_bool.unwrap_or(false),
            n.pressed,
            n.hovered,
        );
        let face = chain
            .iter()
            .find_map(|&state| n.part.and_then(|p| p.face_if(state)))
            .or_else(|| n.part.and_then(|p| p.face(chain[2])));
        if let Some(face) = face {
            self.draw_face(p, face, n.rect, n.clip);
        }
        if let crate::doc::NodeKind::Toggle { icon: Some(icon) } = &inst.node.kind {
            let icon_part = self.theme.part(icon);
            let (icon_w, icon_h) = icon_part.map(Part::natural).unwrap_or((0, 0));
            if let Some(face) = icon_part.and_then(|pt| pt.face(FaceState::Default)) {
                let at = RectI {
                    x: n.rect.x + (n.rect.w - icon_w) / 2,
                    y: n.rect.y + (n.rect.h - icon_h) / 2,
                    w: icon_w,
                    h: icon_h,
                };
                self.draw_sprite(p, face, at, PaintStyle::plain(n.clip));
            }
        }
    }

    pub(super) fn slider(&self, n: &Here<'a>, min: f32, max: f32, p: &mut Painter<'_>) {
        let inst = n.inst;
        let rect = n.rect;
        let track_h = n
            .part
            .map(|p| p.natural().1)
            .filter(|h| *h > 0)
            .unwrap_or(6);
        let track = RectI {
            y: rect.y + (rect.h - track_h) / 2,
            h: track_h,
            ..rect
        };
        if let Some(face) = n.part.and_then(|p| p.face(FaceState::Default)) {
            self.draw_face(p, face, track, n.clip);
        }
        let dragging = matches!(
            &self.fs.drag,
            Some(crate::input::Drag::Slider { key }) if Some(key) == inst.key.as_ref()
        );
        let value = inst.value_f32.unwrap_or(min);
        let handle = widget::slider_handle(rect, self.theme, min, max, value);
        let state = if !inst.enabled {
            FaceState::Disabled
        } else if dragging {
            FaceState::Pressed
        } else if n.hovered {
            FaceState::Hover
        } else {
            FaceState::Default
        };
        if let Some(face) = self.face_of("slider.handle", state) {
            self.draw_sprite(p, face, handle, PaintStyle::plain(n.clip));
        }
    }

    pub(super) fn text_input(&self, n: &Here<'a>, placeholder: Option<&str>, p: &mut Painter<'_>) {
        let inst = n.inst;
        let (rect, clip) = (n.rect, n.clip);
        let state = if !inst.enabled {
            FaceState::Disabled
        } else if n.focused {
            FaceState::Focus
        } else {
            FaceState::Default
        };
        if let Some(face) = n.part.and_then(|p| p.face(state)) {
            self.draw_face(p, face, rect, clip);
        }
        let font = self.theme.ui_font();
        let text_rect = widget::input_text_rect(rect, self.theme.metrics.button_pad);
        let visible = widget::input_visible_chars(font, text_rect.w);
        let ty = rect.y + (rect.h - font.line_h()) / 2;
        let text_color = self.theme.color(palette::TEXT);
        match inst.key.as_ref().and_then(|k| self.fs.editors.get(k)) {
            Some(editor) => {
                let view = editor.render(visible, n.focused, self.fs.now);
                let selection = self.theme.color(palette::SELECTION);
                p.text_input_view(&view, text_rect.x, ty, text_color, selection, clip);
            }
            None => {
                let bound = inst.text.as_deref().unwrap_or("");
                let (shown, color) = if bound.is_empty() {
                    match placeholder {
                        Some(ph) => (ph, self.theme.color(palette::TEXT_MUTED)),
                        None => return,
                    }
                } else {
                    (bound, text_color)
                };
                let shown: String = shown.chars().take(visible).collect();
                p.text(&shown, text_rect.x, ty, color, clip);
            }
        }
    }

    pub(super) fn slots(&self, n: &Here<'a>, cols: u32, rows: u32, p: &mut Painter<'_>) {
        for c in 0..cols * rows {
            let cell = grid_cell(n.rect, cols, c, self.metrics);
            if let Some(face) = n.part.and_then(|p| p.face(FaceState::Default)) {
                self.draw_face(p, face, cell, n.clip);
            }
            // Overlay faces: the bound `selected` cell (hotbar active slot)
            // and the hovered cell.
            let overlay = if n.inst.selected == Some(c as i32) {
                n.part.and_then(|p| {
                    p.face_if(FaceState::Selected)
                        .or_else(|| p.face_if(FaceState::Hover))
                })
            } else if self.slot_hover == Some((n.i, c)) {
                n.part.and_then(|p| p.face_if(FaceState::Hover))
            } else {
                None
            };
            if let Some(face) = overlay {
                self.draw_face(p, face, cell, n.clip);
            }
        }
    }

    pub(super) fn gauge(&self, n: &Here<'a>, mode: GaugeMode, p: &mut Painter<'_>) {
        let rect = n.rect;
        let frac = n.inst.value_f32.unwrap_or(0.0).clamp(0.0, 1.0);
        if let Some(face) = n.part.and_then(|p| p.face(FaceState::Empty)) {
            self.draw_sprite(p, face, rect, PaintStyle::plain(n.clip));
        }
        if frac <= 0.0 {
            return;
        }
        let fill = match mode {
            GaugeMode::GrowLr => RectI {
                w: (rect.w as f32 * frac).round() as i32,
                ..rect
            },
            GaugeMode::DepleteTd => {
                let keep = (rect.h as f32 * frac).round() as i32;
                RectI {
                    y: rect.y + rect.h - keep,
                    h: keep,
                    ..rect
                }
            }
        };
        let fill_clip = match n.clip {
            Some(c) => fill.intersect(c),
            None => fill,
        };
        if let Some(face) = n.part.and_then(|p| p.face(FaceState::Full)) {
            let style = PaintStyle {
                color: n.inst.tint.unwrap_or([1.0; 4]),
                clip: Some(fill_clip),
            };
            self.draw_sprite(p, face, rect, style);
        }
    }

    pub(super) fn tab_bar(&self, n: &Here<'a>, tabs: &[TabSpec], p: &mut Painter<'_>) {
        let inst = n.inst;
        let widths = widget::tab_widths(self.theme, tabs);
        let gap = self.theme.metrics.tab_gap;
        for (t, tab) in tabs.iter().enumerate() {
            let cell = widget::tab_cell(n.rect, &widths, gap, t);
            let state = if !inst.enabled {
                FaceState::Disabled
            } else if inst.selected == Some(t as i32) {
                FaceState::Selected
            } else if self.tab_hover == Some((n.i, t as u32)) {
                FaceState::Hover
            } else {
                FaceState::Default
            };
            if let Some(face) = n.part.and_then(|p| p.face(state)) {
                self.draw_face(p, face, cell, n.clip);
            }
            // Icon + label centred as one block, like leaf buttons.
            let text = tab.label.as_deref().unwrap_or("");
            let x = self.icon_and_label_block(p, cell, tab.icon.as_deref(), text, n.clip);
            if !text.is_empty() {
                let y = cell.y + (cell.h - self.theme.ui_font().line_h()) / 2;
                let color = self.label_color(n.part, inst.enabled);
                p.text(text, x, y, color, n.clip);
            }
        }
    }

    pub(super) fn badge(&self, n: &Here<'a>, p: &mut Painter<'_>) {
        let rect = n.rect;
        if let Some(face) = n.part.and_then(|p| p.face(FaceState::Default)) {
            self.draw_face(p, face, rect, n.clip);
        }
        if let Some(text) = n.inst.text.as_deref() {
            let font = self.theme.ui_font();
            let tw = font.width(text);
            let line = RectI {
                // Centred while it fits; once it does not, the run starts at
                // the edge and ellipsizes into the chip.
                x: rect.x + (rect.w - tw).max(0) / 2,
                y: rect.y + (rect.h - font.line_h()) / 2,
                w: rect.w - (rect.w - tw).max(0) / 2,
                h: font.line_h(),
            };
            let color = self.label_color(n.part, n.inst.enabled);
            p.text_ellipsized(text, line, color, n.clip);
        }
    }

    pub(super) fn alert(&self, n: &Here<'a>, p: &mut Painter<'_>) {
        let (inst, rect) = (n.inst, n.rect);
        let face = n.part.and_then(|p| p.face(FaceState::Default));
        let insets = face.and_then(|f| f.slice).unwrap_or([4, 4, 4, 4]);
        if let Some(face) = face {
            self.draw_face(p, face, rect, n.clip);
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
            if let Some(face) = icon.face(FaceState::Default) {
                let at = RectI {
                    x: tx,
                    y: rect.y + (rect.h - ih) / 2,
                    w: iw,
                    h: ih,
                };
                self.draw_sprite(p, face, at, PaintStyle::plain(n.clip));
            }
            tx += iw + ICON_GAP;
        }
        if let Some(text) = inst.text.as_deref() {
            // Wrap to the frame's interior; centre the wrapped block
            // vertically (single lines land where they always did).
            let font = self.theme.ui_font();
            let text_w = (rect.x + rect.w - insets[2] - tx).max(font.max_advance());
            let (_, block_h) = font.measure(text, Some(text_w));
            let block = RectI {
                x: tx,
                y: rect.y + (rect.h - block_h) / 2,
                w: text_w,
                h: block_h,
            };
            let color = self.label_color(n.part, inst.enabled);
            p.text_wrapped(text, block, color, n.clip);
        }
    }
}
