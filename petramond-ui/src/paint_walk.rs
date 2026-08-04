//! The paint pass: walk the solved instance tree in arena (paint) order and
//! emit every themed quad and glyph into the [`crate::DrawList`].
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

/// The frame an `image`/image-backed `button` shows, from its optional grid
/// `[cols, rows]`, the resolved `bind.frame` value, its `fps`, and the frame
/// clock. A bound frame is authoritative and clamps into the sheet; a
/// positive finite `fps` cycles row-major from `now`; anything else rests on
/// frame 0. `None` grid = a single-frame sheet.
pub(crate) fn frame_index(
    frames: Option<[u32; 2]>,
    bound: Option<i32>,
    fps: Option<f32>,
    now: f64,
) -> u32 {
    let Some([cols, rows]) = frames else {
        return 0;
    };
    let count = (cols as u64 * rows as u64).max(1);
    if let Some(b) = bound {
        return ((b.max(0) as u64).min(count - 1)) as u32;
    }
    match fps {
        Some(fps) if fps > 0.0 && fps.is_finite() => {
            (((now * fps as f64).floor().max(0.0) as u64) % count) as u32
        }
        _ => 0,
    }
}

/// The pixel source rect of the current frame within a sheet of `size`,
/// row-major: the whole sheet when unframed, one grid cell otherwise. This is
/// the ONE place frame geometry is computed — `image` nodes and image-backed
/// buttons both draw through it, so the two can never disagree.
pub(crate) fn frame_src(
    size: (u32, u32),
    frames: Option<[u32; 2]>,
    bound: Option<i32>,
    fps: Option<f32>,
    now: f64,
) -> [u32; 4] {
    let Some([cols, rows]) = frames.filter(|&[c, r]| c > 0 && r > 0) else {
        return [0, 0, size.0, size.1];
    };
    let (fw, fh) = (size.0 / cols, size.1 / rows);
    let i = frame_index(frames, bound, fps, now);
    let (col, row) = (i % cols, i / cols);
    [col * fw, row * fh, fw, fh]
}

/// A framed sheet's natural layout size: ONE frame, not the whole sheet.
pub(crate) fn frame_cell(sheet: (i32, i32), frames: Option<[u32; 2]>) -> (i32, i32) {
    match frames.filter(|&[c, r]| c > 0 && r > 0) {
        Some([c, r]) => (sheet.0 / c as i32, sheet.1 / r as i32),
        None => sheet,
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
    /// Hovered tab cell as `(tab_bar inst, tab)`.
    pub tab_hover: Option<(u32, u32)>,
    pub preview: Option<&'a PreviewState>,
}

impl PaintCtx<'_> {
    pub fn paint(&self, p: &mut Painter<'_>) {
        self.node(ROOT, None, p);
        // Floating tooltips paint last, in their own draw-list tier: the host
        // layers its item icons over the base tier, so a tooltip painted in
        // document order would have those icons showing through it.
        // Unconditional, so "no tooltip this frame" is an EMPTY overlay tier
        // rather than an unset boundary that would read as "all of it".
        p.list.begin_overlay();
        for i in 0..self.tree.len() as u32 {
            if matches!(self.tree.get(i).node.kind, NodeKind::Tooltip { .. }) {
                self.node(i, None, p);
            }
        }
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
            | NodeKind::List { .. }
            | NodeKind::Tooltip { .. }
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
            NodeKind::Label {
                wrap, scale, small, ..
            } => {
                let text = inst.text.as_deref().unwrap_or("");
                let color = label_color(part, inst.enabled);
                if *scale > 1 {
                    p.text_scaled(text, rect.x, rect.y, *scale, color, clip);
                } else if *small && *wrap {
                    p.text_wrapped_small(text, rect, color, clip);
                } else if *small {
                    p.text_ellipsized_small(text, rect, color, clip);
                } else if *wrap {
                    p.text_wrapped(text, rect, color, clip);
                } else {
                    p.text_ellipsized(text, rect, color, clip);
                }
            }
            NodeKind::Image {
                fit, frames, fps, ..
            } => {
                if let Some((tex, size)) = inst.image_name().and_then(|n| self.images.resolve(n)) {
                    let src = frame_src(size, *frames, inst.frame, *fps, self.fs.now);
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
            NodeKind::Button {
                icon, frames, fps, ..
            } => {
                // An image-backed button draws its document image instead of
                // the theme face; state affordance is a plain multiply tint.
                if let Some((tex, size)) = inst.image_name().and_then(|n| self.images.resolve(n)) {
                    let src = frame_src(size, *frames, inst.frame, *fps, self.fs.now);
                    let tint = if !inst.enabled {
                        [0.45, 0.45, 0.45, 1.0]
                    } else if pressed {
                        [0.7, 0.7, 0.7, 1.0]
                    } else if hovered || row_state == Some("selected") {
                        [1.15, 1.15, 1.15, 1.0]
                    } else {
                        [1.0; 4]
                    };
                    p.sprite(TexId::DocImage(tex), rect, src, size, tint, clip);
                } else {
                    let selected = row_state == Some("selected");
                    let state = widget::button_face_state(
                        inst.enabled,
                        selected,
                        pressed,
                        hovered,
                        part.is_some_and(|p| p.face_if("selected").is_some()),
                    );
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
                    let tw = self.theme.ui_font().width(text);
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
                        // Centred while the block fits; once it does not, the run
                        // starts at the padding edge and ellipsizes into the face
                        // instead of painting out of the button.
                        let pad = self.theme.metrics.button_pad;
                        let x = cx.max(rect.x + pad);
                        let line_h = self.theme.ui_font().line_h();
                        let line = RectI {
                            x,
                            y: rect.y + (rect.h - line_h) / 2 + label_off[1],
                            w: (rect.x + rect.w - pad - x).max(0),
                            h: line_h,
                        };
                        p.text_ellipsized(text, line, label_color(part, inst.enabled), clip);
                    }
                }
            }
            NodeKind::Checkbox | NodeKind::Toggle { .. } => {
                let chain = widget::toggle_face_chain(
                    inst.enabled,
                    inst.value_bool.unwrap_or(false),
                    pressed,
                    hovered,
                );
                let face = chain
                    .iter()
                    .find_map(|state| part.and_then(|p| p.face_if(state)))
                    .or_else(|| part.and_then(|p| p.face(chain[2])));
                if let Some(face) = face {
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
                let ty = rect.y + (rect.h - self.theme.ui_font().line_h()) / 2;
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
                            inst.tint.unwrap_or([1.0; 4]),
                            Some(fill_clip),
                        );
                    }
                }
            }
            NodeKind::TabBar { tabs } => {
                let widths = widget::tab_widths(self.theme, tabs);
                let gap = self.theme.metrics.tab_gap;
                for (t, tab) in tabs.iter().enumerate() {
                    let cell = widget::tab_cell(rect, &widths, gap, t);
                    let selected = inst.selected == Some(t as i32);
                    let state = if !inst.enabled {
                        "disabled"
                    } else if selected {
                        "selected"
                    } else if self.tab_hover == Some((i, t as u32)) {
                        "hover"
                    } else {
                        "default"
                    };
                    if let Some(face) = part.and_then(|p| p.face(state)) {
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
                    // Icon + label centred as one block, like leaf buttons.
                    let icon_part = tab.icon.as_deref().and_then(|k| self.theme.part(k));
                    let (icon_w, icon_h) = icon_part.map(|p| p.natural()).unwrap_or((0, 0));
                    let text = tab.label.as_deref().unwrap_or("");
                    let tw = self.theme.ui_font().width(text);
                    let igap = if icon_w > 0 && tw > 0 { 4 } else { 0 };
                    let block_w = icon_w + igap + tw;
                    let mut cx = cell.x + (cell.w - block_w) / 2;
                    if let Some(face) = icon_part.and_then(|pt| pt.face("default")) {
                        p.sprite(
                            TexId::ThemeAtlas,
                            RectI {
                                x: cx,
                                y: cell.y + (cell.h - icon_h) / 2,
                                w: icon_w,
                                h: icon_h,
                            },
                            face.rect,
                            atlas,
                            [1.0; 4],
                            clip,
                        );
                        cx += icon_w + igap;
                    }
                    if !text.is_empty() {
                        p.text(
                            text,
                            cx,
                            cell.y + (cell.h - self.theme.ui_font().line_h()) / 2,
                            label_color(part, inst.enabled),
                            clip,
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
                    let font = self.theme.ui_font();
                    let tw = font.width(text);
                    let line = RectI {
                        // Centred while it fits; once it does not, the run
                        // starts at the edge and ellipsizes into the chip.
                        x: rect.x + (rect.w - tw).max(0) / 2,
                        y: rect.y + (rect.h - font.line_h()) / 2,
                        w: rect.w - (rect.w - tw).max(0) / 2,
                        h: font.line_h(),
                    };
                    p.text_ellipsized(text, line, label_color(part, inst.enabled), clip);
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
                    let text_w =
                        (rect.x + rect.w - insets[2] - tx).max(self.theme.ui_font().cell_w());
                    let (_, block_h) = self.theme.ui_font().measure(text, Some(text_w));
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
        // Tooltip children are skipped here and painted by the overlay pass.
        let is_list = matches!(inst.node.kind, NodeKind::List { .. });
        for (row, &c) in inst.children.iter().enumerate() {
            if matches!(self.tree.get(c).node.kind, NodeKind::Tooltip { .. }) {
                continue;
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use crate::input::{InputEvent, PointerButton};
    use crate::paint::Batch;
    use crate::runtime::{FrameArgs, FrameOutput, UiRuntime};
    use crate::state::{UiState, UiValue};
    use crate::theme::Theme;
    use std::sync::Arc;

    #[test]
    fn bound_frame_is_authoritative_truncates_and_clamps() {
        // 3x2 sheet = 6 frames.
        let g = Some([3, 2]);
        assert_eq!(frame_index(g, Some(2), None, 0.0), 2);
        assert_eq!(
            frame_index(g, Some(0), Some(9.0), 100.0),
            0,
            "bound beats fps"
        );
        // Truncation happens when the binding resolves (f32 -> i32), so the
        // walk only ever sees integers; clamping happens here.
        assert_eq!(frame_index(g, Some(99), None, 0.0), 5, "clamps into sheet");
        assert_eq!(
            frame_index(g, Some(-3), None, 0.0),
            0,
            "negative clamps to 0"
        );
        assert_eq!(frame_index(None, Some(4), None, 0.0), 0, "unframed sheet");
    }

    #[test]
    fn fps_cycles_row_major_and_invalid_rates_rest_on_frame_zero() {
        let g = Some([4, 1]);
        assert_eq!(frame_index(g, None, Some(4.0), 0.0), 0);
        assert_eq!(frame_index(g, None, Some(4.0), 0.3), 1, "floor(1.2)");
        assert_eq!(frame_index(g, None, Some(4.0), 0.6), 2, "floor(2.4)");
        assert_eq!(frame_index(g, None, Some(4.0), 1.1), 0, "floor(4.4) wraps");
        assert_eq!(frame_index(g, None, Some(0.0), 10.0), 0);
        assert_eq!(frame_index(g, None, Some(-1.0), 10.0), 0);
        assert_eq!(frame_index(g, None, None, 10.0), 0);
    }

    #[test]
    fn frame_src_is_row_major_one_cell() {
        // 64x64 sheet, 2x2 grid: frame 2 = row 1, col 0.
        assert_eq!(
            frame_src((64, 64), Some([2, 2]), Some(2), None, 0.0),
            [0, 32, 32, 32]
        );
        assert_eq!(
            frame_src((64, 64), Some([2, 2]), Some(3), None, 0.0),
            [32, 32, 32, 32]
        );
        assert_eq!(frame_src((64, 64), None, None, None, 0.0), [0, 0, 64, 64]);
    }

    // ---- runtime-driven behavior -------------------------------------------------

    struct Sheets(&'static [(&'static str, u16, (u32, u32))]);

    impl DocImages for Sheets {
        fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))> {
            self.0
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, t, s)| (*t, *s))
        }
    }

    fn paint(doc_json: &str, state: &UiState, images: &dyn DocImages, now: f64) -> FrameOutput {
        let rt = UiRuntime::new(
            Arc::new(Document::from_json(doc_json).unwrap()),
            Arc::new(Theme::placeholder()),
        );
        let mut fs = crate::input::FrameState::new();
        let mut out = FrameOutput::default();
        rt.frame(
            FrameArgs {
                screen: (200, 200),
                scale: 1,
                now,
                state,
                input: &[],
                clipboard: None,
                images,
                dim: None,
                preview: None,
            },
            &mut fs,
            &mut out,
        );
        out
    }

    fn doc_image_batch(out: &FrameOutput, tex: u16) -> Option<&Batch> {
        out.draw
            .batches
            .iter()
            .find(|b| b.tex == TexId::DocImage(tex))
    }

    /// The top-left UV of the first quad drawn from document image `tex`.
    fn doc_image_uv0(out: &FrameOutput, tex: u16) -> Option<[f32; 2]> {
        let b = doc_image_batch(out, tex)?;
        Some(out.draw.vertices[b.start as usize].uv)
    }

    const FRAMED_IMAGE_DOC: &str = r#"{
        "format": 1, "kind": "petramond:anim", "class": "screen",
        "root": { "type": "frame", "children": [
            { "type": "image", "image": "flame", "frames": [2, 2], "fps": 4.0,
              "bind": { "frame": "f" } }
        ] }
    }"#;

    #[test]
    fn painted_uvs_follow_the_bound_frame() {
        let images = Sheets(&[("flame", 0, (64, 64))]);
        let mut state = UiState::new();

        state.set("f", UiValue::I32(1));
        let out = paint(FRAMED_IMAGE_DOC, &state, &images, 0.0);
        assert_eq!(
            doc_image_uv0(&out, 0),
            Some([0.5, 0.0]),
            "frame 1 = col 1, row 0"
        );

        // A fractional bound frame truncates (2.9 -> 2 = row 1, col 0).
        state.set("f", UiValue::F32(2.9));
        let out = paint(FRAMED_IMAGE_DOC, &state, &images, 0.0);
        assert_eq!(doc_image_uv0(&out, 0), Some([0.0, 0.5]));

        // Past the end clamps to the last frame instead of wrapping.
        state.set("f", UiValue::I32(99));
        let out = paint(FRAMED_IMAGE_DOC, &state, &images, 0.0);
        assert_eq!(
            doc_image_uv0(&out, 0),
            Some([0.5, 0.5]),
            "clamped to frame 3"
        );

        // Unbound: fps animates from the clock (now 0.6, 4 fps -> frame 2).
        let out = paint(FRAMED_IMAGE_DOC, &UiState::new(), &images, 0.6);
        assert_eq!(doc_image_uv0(&out, 0), Some([0.0, 0.5]));
    }

    const IMAGE_BUTTON_DOC: &str = r#"{
        "format": 1, "kind": "petramond:anim_button", "class": "screen",
        "root": { "type": "frame", "children": [
            { "type": "button", "id": "go", "image": "go_btn", "frames": [2, 2] }
        ] }
    }"#;

    #[test]
    fn image_backed_button_sizes_to_one_frame_and_still_clicks() {
        let images = Sheets(&[("go_btn", 0, (64, 64))]);
        let rt = UiRuntime::new(
            Arc::new(Document::from_json(IMAGE_BUTTON_DOC).unwrap()),
            Arc::new(Theme::placeholder()),
        );
        let state = UiState::new();
        let mut fs = crate::input::FrameState::new();
        let mut out = FrameOutput::default();
        let frame =
            |input: &[InputEvent], fs: &mut crate::input::FrameState, out: &mut FrameOutput| {
                rt.frame(
                    FrameArgs {
                        screen: (200, 200),
                        scale: 1,
                        now: 0.0,
                        state: &state,
                        input,
                        clipboard: None,
                        images: &images,
                        dim: None,
                        preview: None,
                    },
                    fs,
                    out,
                );
            };
        frame(&[], &mut fs, &mut out);

        // Natural size is ONE frame of the 2x2 sheet, not the whole 64x64.
        let r = out.rect("go").expect("button rect");
        assert_eq!((r.w, r.h), (32, 32));

        // It draws its document image and NOT the theme button face: with an
        // unstyled root frame the draw list holds only the image batch.
        assert!(doc_image_batch(&out, 0).is_some());
        assert!(
            out.draw.batches.iter().all(|b| b.tex == TexId::DocImage(0)),
            "no theme chrome behind an image-backed button: {:?}",
            out.draw.batches
        );

        // Click behavior is the ordinary button one (press in, release in).
        let (cx, cy) = ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32);
        let down = InputEvent::PointerDown {
            x: cx,
            y: cy,
            button: PointerButton::Primary,
            shift: false,
            slot_drag: false,
        };
        let up = InputEvent::PointerUp {
            x: cx,
            y: cy,
            button: PointerButton::Primary,
        };
        frame(&[down, up], &mut fs, &mut out);
        assert!(
            out.events
                .iter()
                .any(|e| matches!(e, crate::UiEvent::Click { id, .. } if id == "go")),
            "{:?}",
            out.events
        );
    }

    #[test]
    fn bound_image_overrides_an_image_backed_buttons_sheet() {
        let images = Sheets(&[("go_btn", 0, (64, 64)), ("alt_btn", 1, (32, 32))]);
        let doc = r#"{
            "format": 1, "kind": "petramond:anim_button", "class": "screen",
            "root": { "type": "frame", "children": [
                { "type": "button", "id": "go", "image": "go_btn", "frames": [2, 2],
                  "bind": { "image": "face" } }
            ] }
        }"#;
        let mut state = UiState::new();
        state.set("face", UiValue::Str("alt_btn".into()));
        let out = paint(doc, &state, &images, 0.0);

        // The override sheet is the one drawn (tex 1) and measured: one frame
        // of a 32x32 2x2 sheet is 16x16.
        assert!(doc_image_batch(&out, 1).is_some());
        assert!(doc_image_batch(&out, 0).is_none());
        let r = out.rect("go").expect("button rect");
        assert_eq!((r.w, r.h), (16, 16));
    }
}
