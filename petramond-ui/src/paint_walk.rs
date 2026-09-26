//! The paint pass: walk the solved instance tree in arena (paint) order and
//! emit every themed quad and glyph into the [`crate::DrawList`].
//!
//! Face states resolve here from [`FrameState`] + bindings (hover, pressed,
//! focus, disabled, on/off, selected) — the same inputs interaction uses, so
//! what you see is what clicks. This module resolves each instance's state
//! and walks the tree; `kinds` holds one painter per widget kind.
//!
//! Host-drawn content (item icons, hearts) is NOT painted here; the host
//! layers it over this list using the frame's named rects.

mod kinds;

use crate::doc::{NodeKind, ScrollAxis};
use crate::input::{FrameState, PreviewState};
use crate::layout::{RectI, SlotMetrics, Solved};
use crate::paint::{Fit, PaintStyle, Painter, SpriteSrc, TexId};
use crate::theme::{palette, FaceState, Part, PartFace, Theme};
use crate::tree::{Inst, InstKey, InstTree, ROOT};
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

/// Everything one instance's paint reads, resolved once before the per-kind
/// painter runs.
pub(super) struct Here<'a> {
    pub i: u32,
    pub inst: &'a Inst<'a>,
    pub rect: RectI,
    pub clip: Option<RectI>,
    /// The node's theme part (style override, else the widget default).
    pub part: Option<&'a Part>,
    /// A list stamp's row face (`None` outside a list).
    pub row: Option<FaceState>,
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
}

impl<'a> PaintCtx<'a> {
    pub fn paint(&self, p: &mut Painter<'_>) {
        self.node(ROOT, None, p);
        // The overlay tier paints last: the host layers its base item icons
        // over the base tier, so anything that must sit ABOVE that content —
        // `overlay: true` subtrees, then floating tooltips topmost — paints
        // here instead of in document order. Unconditional, so "nothing
        // raised this frame" is an EMPTY overlay tier rather than an unset
        // boundary that would read as "all of it".
        p.list.begin_overlay();
        for i in 0..self.tree.len() as u32 {
            let inst = self.tree.get(i);
            // A raised subtree's ROOT only (a raised parent paints its
            // children itself), and never a tooltip's interior — tooltips
            // keep their own pass below so they stay topmost.
            let parent_raised = inst
                .parent
                .is_some_and(|par| self.solved.raised[par as usize]);
            if inst.node.overlay && !parent_raised && !self.solved.overlay[i as usize] {
                self.node(i, None, p);
            }
        }
        for i in 0..self.tree.len() as u32 {
            if matches!(self.tree.get(i).node.kind, NodeKind::Tooltip { .. }) {
                self.node(i, None, p);
            }
        }
    }

    /// Resolve instance `i`'s interaction state for paint.
    fn here(&self, i: u32, row: Option<FaceState>) -> Here<'a> {
        let tree: &'a InstTree<'a> = self.tree;
        let inst = tree.get(i);
        let key = inst.key.as_ref();
        let previewed = |forced: fn(&PreviewState) -> Option<&InstKey>| {
            key.is_some() && self.preview.is_some_and(|pv| forced(pv) == key)
        };
        let hovered = self.hover == Some(i) || previewed(|pv| pv.hover.as_ref());
        let pressed = (self.fs.active.as_ref().is_some_and(|(k, _)| Some(k) == key) && hovered)
            // The frame a click fires keeps the pressed face: the host applies
            // the event (e.g. a list selection) only before the NEXT frame, so
            // without this bridge a selected row would flash unpressed for one
            // frame between release and the rebound selection.
            || (key.is_some() && self.fs.clicked.as_ref() == key)
            || previewed(|pv| pv.pressed.as_ref());
        let focused =
            key.is_some() && (self.fs.focus.as_ref() == key || previewed(|pv| pv.focus.as_ref()));
        Here {
            i,
            inst,
            rect: self.solved.rects[i as usize],
            clip: self.solved.clips[i as usize],
            part: self.theme.part_for(inst.node),
            row,
            hovered,
            pressed,
            focused,
        }
    }

    fn node(&self, i: u32, row: Option<FaceState>, p: &mut Painter<'_>) {
        let n = self.here(i, row);
        match &n.inst.node.kind {
            NodeKind::Frame
            | NodeKind::Row
            | NodeKind::Column
            | NodeKind::List { .. }
            | NodeKind::Tooltip { .. }
            | NodeKind::Scroll { .. } => self.container(&n, p),
            NodeKind::Spacer | NodeKind::Hook => {}
            NodeKind::Label {
                wrap, scale, small, ..
            } => self.label(&n, *wrap, *scale, *small, p),
            NodeKind::Image {
                fit, frames, fps, ..
            } => self.image(&n, *fit, *frames, *fps, p),
            NodeKind::Rotimage { pivot, .. } => self.rotimage(&n, *pivot, p),
            NodeKind::Button {
                icon, frames, fps, ..
            } => self.button(&n, icon.as_deref(), *frames, *fps, p),
            NodeKind::Checkbox | NodeKind::Toggle { .. } => self.toggle(&n, p),
            NodeKind::Slider { min, max, .. } => self.slider(&n, *min, *max, p),
            NodeKind::TextInput { placeholder, .. } => {
                self.text_input(&n, placeholder.as_deref(), p)
            }
            NodeKind::Slot { .. } => self.slots(&n, 1, 1, p),
            NodeKind::SlotGrid { cols, rows, .. } => self.slots(&n, *cols, *rows, p),
            NodeKind::Gauge { mode } => self.gauge(&n, *mode, p),
            NodeKind::TabBar { tabs } => self.tab_bar(&n, tabs, p),
            NodeKind::Badge { .. } => self.badge(&n, p),
            NodeKind::Alert { .. } => self.alert(&n, p),
        }
        self.children(&n, p);
        if let NodeKind::Scroll {
            axis: ScrollAxis::Vertical,
        } = n.inst.node.kind
        {
            self.scrollbar(&n, p);
        }
    }

    /// Children in arena order; list stamps carry their row face state.
    /// Tooltip and `overlay: true` children are skipped here and painted by
    /// the overlay pass — unless THIS node is already painting in that pass,
    /// in which case its raised children belong to it.
    fn children(&self, n: &Here<'a>, p: &mut Painter<'_>) {
        let inst = n.inst;
        let is_list = matches!(inst.node.kind, NodeKind::List { .. });
        let self_raised = self.solved.raised[n.i as usize];
        for (row, &c) in inst.children.iter().enumerate() {
            let child = self.tree.get(c);
            if matches!(child.node.kind, NodeKind::Tooltip { .. }) {
                continue;
            }
            if child.node.overlay && !self_raised {
                continue;
            }
            let row_state = is_list.then(|| {
                if !child.enabled {
                    FaceState::Disabled
                } else if inst.selected == Some(row as i32) {
                    FaceState::Selected
                } else if self.row_hover == Some((n.i, row as u32)) {
                    FaceState::Hover
                } else {
                    FaceState::Default
                }
            });
            self.node(c, row_state, p);
        }
    }

    /// Scrollbar chrome over a vertical scroll node's children.
    fn scrollbar(&self, n: &Here<'a>, p: &mut Painter<'_>) {
        let inst = n.inst;
        let content = self.solved.scroll_content[n.i as usize].unwrap_or((0, 0));
        let offset = inst
            .key
            .as_ref()
            .map(|k| self.fs.scroll_offset(k))
            .unwrap_or(0);
        let view = widget::scroll_view_rect(self.theme, inst.node, n.rect);
        let Some((track, thumb)) = widget::scrollbar(
            view,
            n.rect.h,
            content.1,
            offset,
            self.theme.metrics.scrollbar_w,
        ) else {
            return;
        };
        if let Some(face) = self.face_of("scrollbar.track", FaceState::Default) {
            self.draw_face(p, face, track, n.clip);
        }
        let dragging = matches!(
            &self.fs.drag,
            Some(crate::input::Drag::ScrollThumb { key, .. }) if Some(key) == inst.key.as_ref()
        );
        let state = if dragging {
            FaceState::Hover
        } else {
            FaceState::Default
        };
        if let Some(face) = self.face_of("scrollbar.thumb", state) {
            self.draw_face(p, face, thumb, n.clip);
        }
    }

    // ---- shared drawing -------------------------------------------------------

    /// Where a theme face's pixels live.
    pub(super) fn face_src(&self, face: &PartFace) -> SpriteSrc {
        SpriteSrc {
            tex: TexId::ThemePage(face.page),
            rect: face.rect,
            tex_size: self.theme.page_size(face.page),
        }
    }

    /// A theme face 9-sliced over `rect` (plain stretch when it has no
    /// slice insets).
    pub(super) fn draw_face(
        &self,
        p: &mut Painter<'_>,
        face: &PartFace,
        rect: RectI,
        clip: Option<RectI>,
    ) {
        let fit = Fit::NineSlice(face.slice.unwrap_or([0; 4]));
        p.sprite(&self.face_src(face), rect, fit, PaintStyle::plain(clip));
    }

    /// A theme face stretched over `rect` (icons, gauge fills, handles).
    pub(super) fn draw_sprite(
        &self,
        p: &mut Painter<'_>,
        face: &PartFace,
        rect: RectI,
        style: PaintStyle,
    ) {
        p.sprite(&self.face_src(face), rect, Fit::Stretch, style);
    }

    /// Part `key`'s face for `state`, if the theme has the part.
    pub(super) fn face_of(&self, key: &str, state: FaceState) -> Option<&'a PartFace> {
        let theme: &'a Theme = self.theme;
        theme.part(key).and_then(|part| part.face(state))
    }

    /// A document image by name: its texture and size.
    pub(super) fn doc_image(&self, name: Option<&str>) -> Option<(u16, (u32, u32))> {
        name.and_then(|n| self.images.resolve(n))
    }

    /// The label colour of a part: disabled text when disabled, else the
    /// part's `label_color`, else plain text.
    pub(super) fn label_color(&self, part: Option<&Part>, enabled: bool) -> [f32; 4] {
        if !enabled {
            return self.theme.color(palette::TEXT_DISABLED);
        }
        let key = part
            .and_then(|p| p.label_color.as_deref())
            .unwrap_or(palette::TEXT);
        self.theme.color(key)
    }
}

#[cfg(test)]
mod tests;
