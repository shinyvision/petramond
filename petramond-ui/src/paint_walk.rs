mod kinds;

use crate::doc::{NodeKind, ScrollAxis};
use crate::input::{FrameState, PreviewState};
use crate::layout::{RectI, SlotMetrics, Solved};
use crate::paint::{Fit, PaintStyle, Painter, SpriteSrc, TexId};
use crate::theme::{palette, FaceState, Part, PartFace, Theme};
use crate::tree::{Inst, InstKey, InstTree, ROOT};
use crate::widget;

pub trait DocImages {
    fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))>;

    fn scene(&self, _name: &str) -> Option<&SceneView> {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SceneElement {
    Image {
        image: String,
        rect: [f32; 4],
    },
    Sprite {
        image: String,
        center: [f32; 2],
    },
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
        filled: bool,
    },
    Text {
        pos: [f32; 2],
        text: String,
        color: [f32; 4],
        small: bool,
        max_w: Option<f32>,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneView {
    pub offset: [f32; 2],
    pub elements: Vec<SceneElement>,
}

pub struct NoImages;

impl DocImages for NoImages {
    fn resolve(&self, _name: &str) -> Option<(u16, (u32, u32))> {
        None
    }
}

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

fn mask(text: &str) -> String {
    "*".repeat(text.chars().count())
}

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
    pub hover: Option<u32>,
    pub slot_hover: Option<(u32, u32)>,
    pub row_hover: Option<(u32, u32)>,
    pub tab_hover: Option<(u32, u32)>,
    pub preview: Option<&'a PreviewState>,
}

pub(super) struct Here<'a> {
    pub i: u32,
    pub inst: &'a Inst<'a>,
    pub rect: RectI,
    pub clip: Option<RectI>,
    pub part: Option<&'a Part>,
    pub row: Option<FaceState>,
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
}

pub(super) enum Icon<'a> {
    Part {
        face: &'a PartFace,
        size: (i32, i32),
    },
    Doc {
        src: SpriteSrc,
        size: (i32, i32),
    },
}

impl Icon<'_> {
    pub(super) fn size(&self) -> (i32, i32) {
        match self {
            Icon::Part { size, .. } | Icon::Doc { size, .. } => *size,
        }
    }
}

pub fn is_doc_image_icon(name: &str) -> bool {
    name.ends_with(".png")
}

impl<'a> PaintCtx<'a> {
    pub fn paint(&self, p: &mut Painter<'_>) {
        self.node(ROOT, None, p);
        p.list.begin_overlay();
        for i in 0..self.tree.len() as u32 {
            let inst = self.tree.get(i);
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
            NodeKind::Spacer | NodeKind::Hook | NodeKind::Viewport { .. } => {}
            NodeKind::Canvas { .. } => self.canvas(&n, p),
            NodeKind::Label {
                wrap,
                scale,
                small,
                max_lines,
                ..
            } => self.label(&n, *wrap, *scale, *small, *max_lines, p),
            NodeKind::Image {
                fit, frames, fps, ..
            } => self.image(&n, *fit, *frames, *fps, p),
            NodeKind::Rotimage { pivot, .. } => self.rotimage(&n, *pivot, p),
            NodeKind::Button { frames, fps, .. } => self.button(&n, *frames, *fps, p),
            NodeKind::Checkbox | NodeKind::Toggle { .. } => self.toggle(&n, p),
            NodeKind::Slider { min, max, .. } => self.slider(&n, *min, *max, p),
            NodeKind::TextInput {
                placeholder,
                masked,
                ..
            } => self.text_input(&n, placeholder.as_deref(), *masked, p),
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

    pub(super) fn face_src(&self, face: &PartFace) -> SpriteSrc {
        SpriteSrc {
            tex: TexId::ThemePage(face.page),
            rect: face.rect,
            tex_size: self.theme.page_size(face.page),
        }
    }

    pub(super) fn draw_face(
        &self,
        p: &mut Painter<'_>,
        face: &PartFace,
        rect: RectI,
        clip: Option<RectI>,
    ) {
        self.draw_face_styled(p, face, rect, PaintStyle::plain(clip));
    }

    pub(super) fn draw_face_styled(
        &self,
        p: &mut Painter<'_>,
        face: &PartFace,
        rect: RectI,
        style: PaintStyle,
    ) {
        let fit = Fit::NineSlice(face.slice.unwrap_or([0; 4]));
        p.sprite(&self.face_src(face), rect, fit, style);
    }

    pub(super) fn draw_sprite(
        &self,
        p: &mut Painter<'_>,
        face: &PartFace,
        rect: RectI,
        style: PaintStyle,
    ) {
        p.sprite(&self.face_src(face), rect, Fit::Stretch, style);
    }

    pub(super) fn face_of(&self, key: &str, state: FaceState) -> Option<&'a PartFace> {
        let theme: &'a Theme = self.theme;
        theme.part(key).and_then(|part| part.face(state))
    }

    pub(super) fn icon(&self, name: &str) -> Option<Icon<'a>> {
        let theme: &'a Theme = self.theme;
        if let Some(part) = theme.part(name) {
            if let Some(face) = part.face(FaceState::Default) {
                return Some(Icon::Part {
                    face,
                    size: part.natural(),
                });
            }
        }
        let (tex, size) = self.images.resolve(name)?;
        Some(Icon::Doc {
            src: SpriteSrc {
                tex: TexId::DocImage(tex),
                rect: [0, 0, size.0, size.1],
                tex_size: size,
            },
            size: (size.0 as i32, size.1 as i32),
        })
    }

    pub(super) fn draw_icon(
        &self,
        p: &mut Painter<'_>,
        icon: &Icon<'_>,
        at: RectI,
        enabled: bool,
        clip: Option<RectI>,
    ) {
        match icon {
            Icon::Part { face, .. } => self.draw_sprite(p, face, at, PaintStyle::plain(clip)),
            Icon::Doc { src, .. } => {
                let color = if enabled {
                    [1.0; 4]
                } else {
                    [0.45, 0.45, 0.45, 1.0]
                };
                p.sprite(src, at, Fit::Stretch, PaintStyle { color, clip });
            }
        }
    }

    pub(super) fn doc_image(&self, name: Option<&str>) -> Option<(u16, (u32, u32))> {
        name.and_then(|n| self.images.resolve(n))
    }

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
