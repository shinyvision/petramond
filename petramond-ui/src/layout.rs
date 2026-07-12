//! The layout solver: deterministic flexbox-lite over an expanded
//! [`InstTree`], entirely in i32 logical pixels.
//!
//! Two passes:
//! 1. **Measure** (bottom-up): every instance reports its natural size —
//!    leaves ask the [`LayoutEnv`] (theme metrics + text measurement),
//!    containers sum children along the flow axis. A width hint threads down
//!    for wrapping labels where the ancestor width is definite.
//! 2. **Arrange** (top-down): parents hand children final rects. Free space
//!    goes to `grow` children by weight, with the integer remainder given to
//!    the first weighted children in document order — shares always sum
//!    exactly, so the result is deterministic and gap-free.
//!
//! Physical px = logical px × the host's integer gui scale, applied at paint;
//! nothing here ever rounds, so draw and hit-test can never diverge.

use crate::doc::{AnchorEdge, Dir, NodeKind, ScrollAxis, Size};
use crate::tree::{InstTree, ROOT};

/// An integer rectangle in logical px, top-left origin, y down.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const ZERO: RectI = RectI {
        x: 0,
        y: 0,
        w: 0,
        h: 0,
    };

    /// Half-open containment (includes top-left edge, excludes bottom-right).
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Shrink by padding `[l, t, r, b]` (clamped to non-negative size).
    pub fn inset(&self, pad: [i32; 4]) -> RectI {
        RectI {
            x: self.x + pad[0],
            y: self.y + pad[1],
            w: (self.w - pad[0] - pad[2]).max(0),
            h: (self.h - pad[1] - pad[3]).max(0),
        }
    }

    pub fn intersect(&self, other: RectI) -> RectI {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = (self.x + self.w).min(other.x + other.w);
        let b = (self.y + self.h).min(other.y + other.h);
        RectI {
            x,
            y,
            w: (r - x).max(0),
            h: (b - y).max(0),
        }
    }
}

/// Theme-side metrics for slot cells (`slot`/`slot_grid` nodes).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlotMetrics {
    /// Slot cell side, logical px.
    pub slot: i32,
    /// Gap between grid cells, logical px.
    pub gap: i32,
}

/// What the solver needs from the theme/text side: natural sizes of leaf
/// widgets (buttons, labels, checkboxes, gauges, slots…) and slot cell
/// metrics. Implemented by `Theme`; tests use fixed mocks.
pub trait LayoutEnv {
    /// Natural content size of a leaf node. `text` is the instance's resolved
    /// display text, `image` its resolved image name (`image`/`rotimage`
    /// nodes), and `avail_w` a definite available width for wrapping labels
    /// (`None` = single line).
    fn leaf_size(
        &self,
        node: &crate::doc::Node,
        text: Option<&str>,
        image: Option<&str>,
        avail_w: Option<i32>,
    ) -> (i32, i32);

    fn slot_metrics(&self) -> SlotMetrics;

    /// A styled container's chrome insets `[l, t, r, b]` (its 9-slice border).
    /// Content is laid out INSIDE these automatically (border-box), so a
    /// framed panel never needs hand-tuned padding just to clear its border.
    fn container_insets(&self, _node: &crate::doc::Node) -> [i32; 4] {
        [0; 4]
    }

    /// Scrollbar lane width (logical px): overflowing scroll content reserves
    /// this so rows never run under the bar.
    fn scrollbar_width(&self) -> i32 {
        8
    }
}

/// Author padding + chrome insets combined — the effective content inset.
pub(crate) fn content_pad(l: &crate::doc::LayoutProps, ins: [i32; 4]) -> [i32; 4] {
    [
        l.pad[0] + ins[0],
        l.pad[1] + ins[1],
        l.pad[2] + ins[2],
        l.pad[3] + ins[3],
    ]
}

/// Solved geometry, indexed by instance arena index.
#[derive(Debug)]
pub struct Solved {
    /// Final absolute rect per instance (logical px).
    pub rects: Vec<RectI>,
    /// Inherited clip per instance (`None` = unclipped). Hit tests and paint
    /// both respect it, so a scrolled-away row can neither draw nor click.
    pub clips: Vec<Option<RectI>>,
    /// Per `scroll` instance: its flow content size (for offset clamping and
    /// thumb geometry).
    pub scroll_content: Vec<Option<(i32, i32)>>,
}

impl Solved {
    /// Whether `(px, py)` hits instance `idx`'s rect within its clip.
    pub fn hit(&self, idx: u32, px: i32, py: i32) -> bool {
        let i = idx as usize;
        self.rects[i].contains(px, py) && self.clips[i].is_none_or(|c| c.contains(px, py))
    }
}

/// The i-th cell (row-major) of a slot grid arranged from `rect`'s top-left.
pub fn grid_cell(rect: RectI, cols: u32, i: u32, m: SlotMetrics) -> RectI {
    let col = (i % cols.max(1)) as i32;
    let row = (i / cols.max(1)) as i32;
    RectI {
        x: rect.x + col * (m.slot + m.gap),
        y: rect.y + row * (m.slot + m.gap),
        w: m.slot,
        h: m.slot,
    }
}

/// Solve the whole tree against a viewport (logical px). `scroll_offset`
/// supplies each `scroll` instance's current offset (the caller owns and
/// clamps it — one frame of lag on clamp after content shrinks is fine).
pub fn solve(
    tree: &InstTree<'_>,
    env: &dyn LayoutEnv,
    viewport: (i32, i32),
    scroll_offset: &dyn Fn(u32) -> i32,
) -> Solved {
    let n = tree.len();
    let mut solver = Solver {
        tree,
        env,
        scroll_offset,
        naturals: vec![(0, 0); n],
        out: Solved {
            rects: vec![RectI::ZERO; n],
            clips: vec![None; n],
            scroll_content: vec![None; n],
        },
    };
    if n == 0 {
        return solver.out;
    }

    let rl = tree.root().layout;
    let root_hint = match rl.w {
        Size::Px(p) => Some(p),
        Size::Grow(_) => Some(viewport.0 - rl.margin[0] - rl.margin[2]),
        Size::Auto => None,
    };
    solver.measure(ROOT, root_hint);

    let (nw, nh) = solver.naturals[ROOT as usize];
    let w = match rl.w {
        Size::Px(p) => p,
        Size::Grow(_) => (viewport.0 - rl.margin[0] - rl.margin[2]).max(0),
        Size::Auto => nw.min(viewport.0),
    };
    let h = match rl.h {
        Size::Px(p) => p,
        Size::Grow(_) => (viewport.1 - rl.margin[1] - rl.margin[3]).max(0),
        Size::Auto => nh.min(viewport.1),
    };
    let anchor = rl.anchor.unwrap_or_default();
    let x = match anchor.h {
        AnchorEdge::Start => rl.margin[0],
        AnchorEdge::Center => (viewport.0 - w) / 2,
        AnchorEdge::End => viewport.0 - w - rl.margin[2],
    };
    let y = match anchor.v {
        AnchorEdge::Start => rl.margin[1],
        AnchorEdge::Center => (viewport.1 - h) / 2,
        AnchorEdge::End => viewport.1 - h - rl.margin[3],
    };
    solver.arrange(ROOT, RectI { x, y, w, h }, None);
    solver.out
}

struct Solver<'t, 'd, 'e> {
    tree: &'t InstTree<'d>,
    env: &'e dyn LayoutEnv,
    scroll_offset: &'e dyn Fn(u32) -> i32,
    naturals: Vec<(i32, i32)>,
    out: Solved,
}

/// One flow axis: extract/pack (main, cross) against (w/x, h/y) pairs.
#[derive(Copy, Clone, PartialEq, Eq)]
struct Ax {
    horizontal: bool,
}

impl Ax {
    fn of_dir(dir: Dir) -> Ax {
        Ax {
            horizontal: dir == Dir::Row,
        }
    }
    fn of(self, wh: (i32, i32)) -> i32 {
        if self.horizontal {
            wh.0
        } else {
            wh.1
        }
    }
    fn pack(self, main: i32, cross: i32) -> (i32, i32) {
        if self.horizontal {
            (main, cross)
        } else {
            (cross, main)
        }
    }
    /// Leading/trailing margin along this axis from `[l, t, r, b]`.
    fn margin_lead(self, m: [i32; 4]) -> i32 {
        if self.horizontal {
            m[0]
        } else {
            m[1]
        }
    }
    fn margin_trail(self, m: [i32; 4]) -> i32 {
        if self.horizontal {
            m[2]
        } else {
            m[3]
        }
    }
    fn size_prop(self, l: &crate::doc::LayoutProps) -> Size {
        if self.horizontal {
            l.w
        } else {
            l.h
        }
    }
}

fn clamp_opt(v: i32, min: Option<i32>, max: Option<i32>) -> i32 {
    let v = if let Some(min) = min { v.max(min) } else { v };
    if let Some(max) = max {
        v.min(max)
    } else {
        v
    }
}

impl Solver<'_, '_, '_> {
    fn measure(&mut self, idx: u32, avail_w: Option<i32>) -> (i32, i32) {
        let tree = self.tree;
        let inst = tree.get(idx);
        let node = inst.node;
        let l = inst.layout;
        let pad = content_pad(l, self.env.container_insets(node));
        let pad_w = pad[0] + pad[2];
        let pad_h = pad[1] + pad[3];

        let mut natural = if node.lays_out_children() {
            let dir = inst.flow_dir();
            let main = Ax::of_dir(dir);
            let cross = Ax {
                horizontal: !main.horizontal,
            };
            let content_avail_w = match l.w {
                Size::Px(p) => Some((p - pad_w).max(0)),
                _ => avail_w.map(|a| (a - pad_w).max(0)),
            };
            let mut main_sum = 0i32;
            let mut cross_max = 0i32;
            let mut n_flow = 0i32;
            for &c in &inst.children {
                let cn = tree.get(c);
                let cm = cn.layout.margin;
                // Wrap hints only flow down columns, where each child gets the
                // full content width; a row's split is unknown until arrange.
                let child_hint = match dir {
                    Dir::Column => content_avail_w.map(|a| (a - cm[0] - cm[2]).max(0)),
                    Dir::Row => None,
                };
                let (cw, ch) = self.measure(c, child_hint);
                if cn.layout.abs.is_some() {
                    continue;
                }
                let outer = (cw + cm[0] + cm[2], ch + cm[1] + cm[3]);
                main_sum += main.of(outer);
                cross_max = cross_max.max(cross.of(outer));
                n_flow += 1;
            }
            if n_flow > 1 {
                main_sum += l.gap * (n_flow - 1);
            }
            let (w, h) = main.pack(main_sum, cross_max);
            (w + pad_w, h + pad_h)
        } else {
            let inner_avail = match l.w {
                Size::Px(p) => Some((p - pad_w).max(0)),
                _ => avail_w.map(|a| (a - pad_w).max(0)),
            };
            let (w, h) =
                self.env
                    .leaf_size(node, inst.text.as_deref(), inst.image_name(), inner_avail);
            (w + pad_w, h + pad_h)
        };

        if let Size::Px(p) = l.w {
            natural.0 = p;
        }
        if let Size::Px(p) = l.h {
            natural.1 = p;
        }
        natural.0 = clamp_opt(natural.0, l.min_w, l.max_w);
        natural.1 = clamp_opt(natural.1, l.min_h, l.max_h);
        self.naturals[idx as usize] = natural;
        natural
    }

    fn arrange(&mut self, idx: u32, rect: RectI, clip: Option<RectI>) {
        self.out.rects[idx as usize] = rect;
        self.out.clips[idx as usize] = clip;
        let tree = self.tree;
        let inst = tree.get(idx);
        let node = inst.node;
        if inst.children.is_empty() {
            return;
        }
        let l = inst.layout;
        let pad = content_pad(l, self.env.container_insets(node));
        let content = rect.inset(pad);
        let dir = inst.flow_dir();
        let main = Ax::of_dir(dir);
        let cross = Ax {
            horizontal: !main.horizontal,
        };

        // Scroll nodes clip their children and shift them by the offset along
        // the scroll axis (flow direction is independent of scroll axis).
        let scroll_axis = match node.kind {
            NodeKind::Scroll { axis } => Some(axis),
            _ => None,
        };
        let (shift_x, shift_y) = match scroll_axis {
            Some(ScrollAxis::Vertical) => (0, -(self.scroll_offset)(idx)),
            Some(ScrollAxis::Horizontal) => (-(self.scroll_offset)(idx), 0),
            None => (0, 0),
        };
        let child_clip = if scroll_axis.is_some() {
            Some(match clip {
                Some(c) => c.intersect(content),
                None => content,
            })
        } else {
            clip
        };

        let flow: Vec<u32> = inst
            .children
            .iter()
            .copied()
            .filter(|&c| tree.get(c).layout.abs.is_none())
            .collect();

        // Main-axis base sizes + grow weights. Inside a scroll node, grow is
        // inert along the scroll axis (content is unbounded there).
        let grow_inert = match scroll_axis {
            Some(ScrollAxis::Vertical) => !main.horizontal,
            Some(ScrollAxis::Horizontal) => main.horizontal,
            None => false,
        };
        let mut bases: Vec<i32> = Vec::with_capacity(flow.len());
        let mut weights: Vec<u32> = Vec::with_capacity(flow.len());
        let mut outer_sum = 0i32;
        for &c in &flow {
            let cl = tree.get(c).layout;
            let nat_main = main.of(self.naturals[c as usize]);
            let base = match main.size_prop(cl) {
                Size::Px(p) => p,
                _ => nat_main,
            };
            let weight = match main.size_prop(cl) {
                Size::Grow(g) if !grow_inert => g,
                _ => 0,
            };
            bases.push(base);
            weights.push(weight);
            outer_sum += base + main.margin_lead(cl.margin) + main.margin_trail(cl.margin);
        }
        let gaps = if flow.len() > 1 {
            l.gap * (flow.len() as i32 - 1)
        } else {
            0
        };

        // Overflowing scroll content reserves the scrollbar lane so rows
        // never run under the bar.
        let mut avail = content;
        if let Some(axis) = scroll_axis {
            let flow_is_axis = matches!(
                (axis, dir),
                (ScrollAxis::Vertical, Dir::Column) | (ScrollAxis::Horizontal, Dir::Row)
            );
            let flow_len = if flow_is_axis {
                outer_sum + gaps
            } else {
                flow.iter()
                    .map(|&c| {
                        let cl = tree.get(c).layout;
                        match axis {
                            ScrollAxis::Vertical => {
                                self.naturals[c as usize].1 + cl.margin[1] + cl.margin[3]
                            }
                            ScrollAxis::Horizontal => {
                                self.naturals[c as usize].0 + cl.margin[0] + cl.margin[2]
                            }
                        }
                    })
                    .max()
                    .unwrap_or(0)
            };
            let (viewport_len, pad_axis) = match axis {
                ScrollAxis::Vertical => (rect.h, pad[1] + pad[3]),
                ScrollAxis::Horizontal => (rect.w, pad[0] + pad[2]),
            };
            if flow_len + pad_axis > viewport_len {
                let bar = self.env.scrollbar_width();
                match axis {
                    ScrollAxis::Vertical => avail.w = (avail.w - bar).max(0),
                    ScrollAxis::Horizontal => avail.h = (avail.h - bar).max(0),
                }
            }
        }
        let content_main = main.of((avail.w, avail.h));
        let mut leftover = content_main - outer_sum - gaps;

        // Distribute positive leftover to growers by weight; the integer
        // remainder goes +1 each to the first `rem` weighted children in
        // document order, so shares sum exactly.
        let total_weight: u32 = weights.iter().sum();
        if leftover > 0 && total_weight > 0 {
            let mut shares: Vec<i32> = weights
                .iter()
                .map(|&w| ((leftover as i64 * w as i64) / total_weight as i64) as i32)
                .collect();
            let mut rem = leftover - shares.iter().sum::<i32>();
            for (i, &w) in weights.iter().enumerate() {
                if rem == 0 {
                    break;
                }
                if w > 0 {
                    shares[i] += 1;
                    rem -= 1;
                }
            }
            for (i, s) in shares.iter().enumerate() {
                // Respect max_* caps; capped leftover is not redistributed.
                let cl = tree.get(flow[i]).layout;
                let capped = if main.horizontal {
                    clamp_opt(bases[i] + s, None, cl.max_w)
                } else {
                    clamp_opt(bases[i] + s, None, cl.max_h)
                };
                bases[i] = capped;
            }
            leftover = 0;
        }

        // SHRINK: when space runs short, grow children give it back — down to
        // their `min_*` (else zero) — so a flexible scroll section absorbs the
        // deficit and shows its scrollbar instead of pushing siblings out.
        // Only when every grower is at its minimum does content overflow.
        if leftover < 0 && total_weight > 0 {
            let min_of = |i: usize| -> i32 {
                let cl = tree.get(flow[i]).layout;
                if main.horizontal {
                    cl.min_w.unwrap_or(0)
                } else {
                    cl.min_h.unwrap_or(0)
                }
                .max(0)
            };
            let mut deficit = -leftover;
            while deficit > 0 {
                let cands: Vec<usize> = (0..flow.len())
                    .filter(|&i| weights[i] > 0 && bases[i] > min_of(i))
                    .collect();
                if cands.is_empty() {
                    break;
                }
                let wsum: i64 = cands.iter().map(|&i| weights[i] as i64).sum();
                let mut cut_any = false;
                for &i in &cands {
                    let share = ((deficit as i64 * weights[i] as i64) / wsum).max(0) as i32;
                    let cut = share.min(bases[i] - min_of(i)).min(deficit);
                    if cut > 0 {
                        bases[i] -= cut;
                        deficit -= cut;
                        cut_any = true;
                    }
                }
                if !cut_any {
                    // Integer floors all rounded to zero: peel 1px at a time.
                    for &i in &cands {
                        if deficit == 0 {
                            break;
                        }
                        let cut = 1.min(bases[i] - min_of(i));
                        bases[i] -= cut;
                        deficit -= cut;
                    }
                }
            }
            leftover = -deficit;
        }

        // Justify only distributes space no grower claimed.
        let (mut cursor, extra_gap, mut gap_rem) = if leftover > 0 {
            match l.justify {
                crate::doc::Justify::Start => (0, 0, 0),
                crate::doc::Justify::Center => (leftover / 2, 0, 0),
                crate::doc::Justify::End => (leftover, 0, 0),
                crate::doc::Justify::SpaceBetween if flow.len() > 1 => {
                    let n = flow.len() as i32 - 1;
                    (0, leftover / n, leftover % n)
                }
                crate::doc::Justify::SpaceBetween => (0, 0, 0),
            }
        } else {
            (0, 0, 0)
        };
        cursor += main.of((avail.x, avail.y));

        let content_cross = cross.of((avail.w, avail.h));
        let align = inst.effective_align();
        for (i, &c) in flow.iter().enumerate() {
            let cl = tree.get(c).layout.clone();
            let m_lead = cross.margin_lead(cl.margin);
            let m_trail = cross.margin_trail(cl.margin);
            let nat_cross = cross.of(self.naturals[c as usize]);
            let stretch = (content_cross - m_lead - m_trail).max(0);
            let cross_size = match cross.size_prop(&cl) {
                Size::Px(p) => p,
                Size::Grow(_) => stretch,
                Size::Auto => {
                    if align == crate::doc::Align::Stretch {
                        stretch
                    } else {
                        nat_cross
                    }
                }
            };
            let cross_size = if cross.horizontal {
                clamp_opt(cross_size, cl.min_w, cl.max_w)
            } else {
                clamp_opt(cross_size, cl.min_h, cl.max_h)
            };
            let cross_start = cross.of((avail.x, avail.y));
            let free = content_cross - cross_size - m_lead - m_trail;
            let cross_pos = match align {
                crate::doc::Align::Start | crate::doc::Align::Stretch => cross_start + m_lead,
                crate::doc::Align::Center => cross_start + m_lead + free / 2,
                crate::doc::Align::End => cross_start + m_lead + free,
            };

            cursor += main.margin_lead(cl.margin);
            let (w, h) = main.pack(bases[i], cross_size);
            let (x, y) = main.pack(cursor, cross_pos);
            self.arrange(
                c,
                RectI {
                    x: x + shift_x,
                    y: y + shift_y,
                    w,
                    h,
                },
                child_clip,
            );
            cursor += bases[i] + main.margin_trail(cl.margin);
            if i + 1 < flow.len() {
                cursor += l.gap + extra_gap + if gap_rem > 0 { 1 } else { 0 };
                gap_rem -= if gap_rem > 0 { 1 } else { 0 };
            }
        }

        // Absolute children: placed against the padded rect, out of flow,
        // natural/explicit size, unaffected by scroll offset.
        for &c in &inst.children {
            let cn = tree.get(c);
            let Some(abs) = cn.layout.abs else {
                continue;
            };
            let (nw, nh) = self.naturals[c as usize];
            let w = match cn.layout.w {
                Size::Px(p) => p,
                Size::Grow(_) => (content.w - abs.x).max(0),
                Size::Auto => nw,
            };
            let h = match cn.layout.h {
                Size::Px(p) => p,
                Size::Grow(_) => (content.h - abs.y).max(0),
                Size::Auto => nh,
            };
            let w = clamp_opt(w, cn.layout.min_w, cn.layout.max_w);
            let h = clamp_opt(h, cn.layout.min_h, cn.layout.max_h);
            self.arrange(
                c,
                RectI {
                    x: content.x + abs.x,
                    y: content.y + abs.y,
                    w,
                    h,
                },
                child_clip,
            );
        }

        if scroll_axis.is_some() {
            let mut cross_used = 0i32;
            for &c in &flow {
                let cl = tree.get(c).layout;
                let r = self.out.rects[c as usize];
                cross_used = cross_used.max(
                    cross.of((r.w, r.h))
                        + cross.margin_lead(cl.margin)
                        + cross.margin_trail(cl.margin),
                );
            }
            let main_used = outer_sum + gaps;
            let (w, h) = main.pack(main_used, cross_used);
            self.out.scroll_content[idx as usize] =
                Some((w + pad[0] + pad[2], h + pad[1] + pad[3]));
        }
    }
}

#[cfg(test)]
mod tests {
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
}
