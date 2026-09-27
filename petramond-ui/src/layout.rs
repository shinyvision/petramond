//! Flexbox-lite layout over an expanded [`InstTree`], i32 logical pixels.
//!
//! Two passes:
//! 1. Measure, bottom-up. Leaves ask [`LayoutEnv`] for natural size, containers sum children along
//!    flow axis. Where the ancestor width is definite, a width hint threads down for wrapping
//!    labels.
//! 2. Arrange, top-down. Parents hand children final rects. Free space splits across grow children
//!    by weight, remainder goes to first weighted children in doc order so shares always sum
//!    exactly.
//!
//! Physical px = logical px * host's integer gui scale, applied at paint. Nothing rounds here, so
//! draw and hit-test can't diverge.

use crate::doc::{AnchorEdge, Dir, NodeKind, ScrollAxis, Size};
use crate::tree::{InstTree, ROOT};

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

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlotMetrics {
    pub slot: i32,
    pub gap: i32,
}

pub trait LayoutEnv {
    fn leaf_size(
        &self,
        node: &crate::doc::Node,
        text: Option<&str>,
        image: Option<&str>,
        avail_w: Option<i32>,
    ) -> (i32, i32);

    fn slot_metrics(&self) -> SlotMetrics;

    fn gui_scale(&self) -> i32 {
        1
    }

    fn container_insets(&self, _node: &crate::doc::Node) -> [i32; 4] {
        [0; 4]
    }

    fn scrollbar_width(&self) -> i32 {
        8
    }
}

pub(crate) fn content_pad(l: &crate::doc::LayoutProps, ins: [i32; 4]) -> [i32; 4] {
    [
        l.pad[0] + ins[0],
        l.pad[1] + ins[1],
        l.pad[2] + ins[2],
        l.pad[3] + ins[3],
    ]
}

#[derive(Clone, Debug)]
pub struct Solved {
    pub rects: Vec<RectI>,
    pub clips: Vec<Option<RectI>>,
    pub scroll_content: Vec<Option<(i32, i32)>>,
    pub overlay: Vec<bool>,
    pub raised: Vec<bool>,
}

impl Solved {
    pub fn hit(&self, idx: u32, px: i32, py: i32) -> bool {
        let i = idx as usize;
        self.rects[i].contains(px, py) && self.clips[i].is_none_or(|c| c.contains(px, py))
    }
}

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
        in_overlay: false,
        in_raised: false,
        out: Solved {
            rects: vec![RectI::ZERO; n],
            clips: vec![None; n],
            scroll_content: vec![None; n],
            overlay: vec![false; n],
            raised: vec![false; n],
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
    in_overlay: bool,
    in_raised: bool,
    out: Solved,
}

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

fn effective_min_w(inst: &crate::tree::Inst<'_>) -> Option<i32> {
    match (inst.layout.min_w, inst.min_w) {
        (Some(authored), Some(bound)) => Some(authored.max(bound)),
        (authored, bound) => authored.or(bound),
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

fn wrap_hint(l: &crate::doc::LayoutProps, pad_w: i32, avail_w: Option<i32>) -> Option<i32> {
    let hint = match l.w {
        Size::Px(p) => Some(p),
        _ => avail_w,
    };
    let hint = match (hint, l.max_w) {
        (Some(a), Some(m)) => Some(a.min(m)),
        (None, Some(m)) => Some(m),
        (a, None) => a,
    };
    hint.map(|a| (a - pad_w).max(0))
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

        let cols = node.kind.list_cols();
        let mut natural = if node.lays_out_children() && cols > 1 {
            let content_avail_w = wrap_hint(l, pad_w, avail_w);
            let cols_i = cols as i32;
            let cell_hint = content_avail_w.map(|a| ((a - l.gap * (cols_i - 1)) / cols_i).max(0));
            let (mut cell_w, mut cell_h) = (0i32, 0i32);
            for &c in &inst.children {
                let (cw, ch) = self.measure(c, cell_hint);
                cell_w = cell_w.max(cw);
                cell_h = cell_h.max(ch);
            }
            let n = inst.children.len() as i32;
            let rows = (n + cols_i - 1) / cols_i;
            let w = if n == 0 {
                0
            } else {
                cell_w * cols_i + l.gap * (cols_i - 1)
            };
            let h = if rows == 0 {
                0
            } else {
                cell_h * rows + l.gap * (rows - 1)
            };
            (w + pad_w, h + pad_h)
        } else if node.lays_out_children() {
            let dir = inst.flow_dir();
            let main = Ax::of_dir(dir);
            let cross = Ax {
                horizontal: !main.horizontal,
            };
            let content_avail_w = wrap_hint(l, pad_w, avail_w);
            let mut main_sum = 0i32;
            let mut cross_max = 0i32;
            let mut n_flow = 0i32;
            for &c in &inst.children {
                let cn = tree.get(c);
                let cm = cn.layout.margin;
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
            if let (Dir::Row, Some(avail)) = (dir, content_avail_w) {
                if main_sum > avail {
                    cross_max = self.narrowed_row_height(idx, avail, cross_max);
                }
            }
            let (w, h) = main.pack(main_sum, cross_max);
            (w + pad_w, h + pad_h)
        } else {
            let inner_avail = wrap_hint(l, pad_w, avail_w);
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
        let content_h = natural.1;
        if matches!(
            node.kind,
            NodeKind::Scroll {
                axis: ScrollAxis::Horizontal
            }
        ) && matches!(l.h, Size::Auto)
            && l.reserve_scrollbar
        {
            natural.1 += self.env.scrollbar_width();
        }
        natural.1 = clamp_opt(natural.1, l.min_h, l.max_h);
        if matches!(
            node.kind,
            NodeKind::Scroll {
                axis: ScrollAxis::Vertical
            }
        ) && matches!(l.w, Size::Auto)
            && (natural.1 < content_h || l.reserve_scrollbar)
        {
            natural.0 += self.env.scrollbar_width();
        }
        natural.0 = clamp_opt(natural.0, effective_min_w(inst), l.max_w);
        self.naturals[idx as usize] = natural;
        natural
    }

    fn arrange(&mut self, idx: u32, rect: RectI, clip: Option<RectI>) {
        let tree = self.tree;
        let inst = tree.get(idx);
        let node = inst.node;
        self.out.rects[idx as usize] = rect;
        self.out.clips[idx as usize] = clip;
        self.out.overlay[idx as usize] = self.in_overlay;
        let was_raised = self.in_raised;
        self.in_raised = self.in_raised || self.in_overlay || node.overlay;
        self.out.raised[idx as usize] = self.in_raised;
        self.arrange_children(idx, rect, clip);
        self.in_raised = was_raised;
    }

    fn narrowed_row_height(&mut self, idx: u32, avail: i32, mut height: i32) -> i32 {
        let tree = self.tree;
        let inst = tree.get(idx);
        let flow: Vec<u32> = inst
            .children
            .iter()
            .copied()
            .filter(|&c| tree.get(c).layout.abs.is_none() && !is_tooltip(tree, c))
            .collect();
        let main = Ax { horizontal: true };
        let (mut bases, weights, outer_sum) = self.flow_bases(&flow, main, false);
        let gaps = inst.layout.gap * (flow.len() as i32 - 1).max(0);
        self.distribute(
            &flow,
            main,
            &mut bases,
            weights,
            avail - outer_sum - gaps,
            false,
        );
        for (&c, &share) in flow.iter().zip(&bases) {
            if share < self.naturals[c as usize].0 {
                let margin = tree.get(c).layout.margin;
                let (_, h) = self.measure(c, Some(share.max(0)));
                height = height.max(h + margin[1] + margin[3]);
            }
        }
        height
    }

    fn flow_bases(&self, flow: &[u32], main: Ax, grow_inert: bool) -> (Vec<i32>, Vec<u32>, i32) {
        let tree = self.tree;
        let mut bases: Vec<i32> = Vec::with_capacity(flow.len());
        let mut weights: Vec<u32> = Vec::with_capacity(flow.len());
        let mut outer_sum = 0i32;
        for &c in flow {
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
        (bases, weights, outer_sum)
    }

    /// Hand `leftover` main-axis space to the flow: growers take a surplus by
    /// weight, and a deficit is taken back from growers, then from
    /// ellipsizable text. Answers what is left over (negative = overflow).
    /// Measure and arrange split a row the same way through here. Along a
    /// scroll's axis (`scrolling`) nothing grows or shrinks: content there is
    /// unbounded, and a squeezed child would spill over the ones after it.
    fn distribute(
        &self,
        flow: &[u32],
        main: Ax,
        bases: &mut [i32],
        weights: Vec<u32>,
        mut leftover: i32,
        scrolling: bool,
    ) -> i32 {
        if scrolling {
            return leftover;
        }
        let tree = self.tree;

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
        // When every grower is at its minimum, ELLIPSIZABLE TEXT gives back
        // next (see [`text_shrinkable`]): a long name shortens instead of
        // shoving the row's widgets off the panel. Only when neither is left
        // does content overflow.
        if leftover < 0 {
            let min_of = |i: usize| -> i32 {
                let ci = tree.get(flow[i]);
                if main.horizontal {
                    effective_min_w(ci).unwrap_or(0)
                } else {
                    ci.layout.min_h.unwrap_or(0)
                }
                .max(0)
            };
            let mut deficit = -leftover;
            let mut weights = weights;
            for i in 0..flow.len() {
                if weights[i] == 0 && grower_inside(tree, flow[i], main.horizontal) {
                    weights[i] = 1;
                }
            }
            let text_claims: Vec<u32> = match main.horizontal {
                true => (0..flow.len())
                    .map(|i| u32::from(weights[i] == 0 && text_shrinkable(tree, flow[i])))
                    .collect(),
                false => Vec::new(),
            };
            for claims in [&weights, &text_claims] {
                if claims.is_empty() {
                    continue;
                }
                let claim = |i: usize| claims[i];
                while deficit > 0 {
                    let cands: Vec<usize> = (0..flow.len())
                        .filter(|&i| claim(i) > 0 && bases[i] > min_of(i))
                        .collect();
                    if cands.is_empty() {
                        break;
                    }
                    let wsum: i64 = cands.iter().map(|&i| claim(i) as i64).sum();
                    let mut cut_any = false;
                    for &i in &cands {
                        let share = ((deficit as i64 * claim(i) as i64) / wsum).max(0) as i32;
                        let cut = share.min(bases[i] - min_of(i)).min(deficit);
                        if cut > 0 {
                            bases[i] -= cut;
                            deficit -= cut;
                            cut_any = true;
                        }
                    }
                    if !cut_any {
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
            }
            leftover = -deficit;
        }
        leftover
    }

    fn arrange_children(&mut self, idx: u32, rect: RectI, clip: Option<RectI>) {
        let tree = self.tree;
        let inst = tree.get(idx);
        let node = inst.node;
        if inst.children.is_empty() {
            return;
        }
        let l = inst.layout;
        let pad = content_pad(l, self.env.container_insets(node));
        let content = rect.inset(pad);
        if node.kind.list_cols() > 1 {
            self.arrange_grid(idx, content, clip);
            return;
        }
        let dir = inst.flow_dir();
        let main = Ax::of_dir(dir);
        let cross = Ax {
            horizontal: !main.horizontal,
        };

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
            .filter(|&c| tree.get(c).layout.abs.is_none() && !is_tooltip(tree, c))
            .collect();

        let grow_inert = match scroll_axis {
            Some(ScrollAxis::Vertical) => !main.horizontal,
            Some(ScrollAxis::Horizontal) => main.horizontal,
            None => false,
        };
        let (mut bases, weights, mut outer_sum) = self.flow_bases(&flow, main, grow_inert);
        let gaps = if flow.len() > 1 {
            l.gap * (flow.len() as i32 - 1)
        } else {
            0
        };

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
            if l.reserve_scrollbar || flow_len + pad_axis > viewport_len {
                let bar = self.env.scrollbar_width();
                match axis {
                    ScrollAxis::Vertical => avail.w = (avail.w - bar).max(0),
                    ScrollAxis::Horizontal => avail.h = (avail.h - bar).max(0),
                }
            }
        }
        if matches!(scroll_axis, Some(ScrollAxis::Vertical))
            && dir == Dir::Column
            && avail.w < content.w
        {
            outer_sum = 0;
            for (i, &c) in flow.iter().enumerate() {
                let cl = tree.get(c).layout;
                let (_, h) = self.measure(c, Some((avail.w - cl.margin[0] - cl.margin[2]).max(0)));
                bases[i] = h;
                outer_sum += h + cl.margin[1] + cl.margin[3];
            }
        }
        let content_main = main.of((avail.w, avail.h));
        let leftover = self.distribute(
            &flow,
            main,
            &mut bases,
            weights,
            content_main - outer_sum - gaps,
            grow_inert,
        );

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
                    } else if cross.horizontal && text_shrinkable(tree, c) {
                        nat_cross.min(stretch)
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

        for &c in &inst.children {
            if !is_tooltip(tree, c) {
                continue;
            }
            let (nw, nh) = self.naturals[c as usize];
            let was = std::mem::replace(&mut self.in_overlay, true);
            self.arrange(
                c,
                RectI {
                    x: content.x,
                    y: content.y,
                    w: nw,
                    h: nh,
                },
                None,
            );
            self.in_overlay = was;
        }

        for &c in &inst.children {
            let cn = tree.get(c);
            let Some(authored) = cn.layout.abs else {
                continue;
            };
            if is_tooltip(tree, c) {
                continue;
            }
            let abs = crate::doc::AbsPos {
                x: cn.abs_x.unwrap_or(authored.x),
                y: cn.abs_y.unwrap_or(authored.y),
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

    fn arrange_grid(&mut self, idx: u32, content: RectI, clip: Option<RectI>) {
        let tree = self.tree;
        let inst = tree.get(idx);
        let cols = inst.node.kind.list_cols() as i32;
        let gap = inst.layout.gap;
        let children: Vec<u32> = inst.children.clone();

        let inner_w = (content.w - gap * (cols - 1)).max(0);
        let base_w = inner_w / cols;
        let extra = inner_w - base_w * cols;
        let col_w = |col: i32| base_w + i32::from(col < extra);
        let col_x = |col: i32| content.x + base_w * col + extra.min(col) + gap * col;
        let cell_h = children
            .iter()
            .map(|&c| self.naturals[c as usize].1)
            .max()
            .unwrap_or(0);

        for (i, &c) in children.iter().enumerate() {
            let (col, row) = (i as i32 % cols, i as i32 / cols);
            self.arrange(
                c,
                RectI {
                    x: col_x(col),
                    y: content.y + row * (cell_h + gap),
                    w: col_w(col),
                    h: cell_h,
                },
                clip,
            );
        }
    }
}

fn is_tooltip(tree: &InstTree<'_>, c: u32) -> bool {
    matches!(tree.get(c).node.kind, NodeKind::Tooltip { .. })
}

fn grower_inside(tree: &InstTree<'_>, c: u32, horizontal: bool) -> bool {
    let inst = tree.get(c);
    let size = match horizontal {
        true => inst.layout.w,
        false => inst.layout.h,
    };
    match size {
        Size::Px(_) => false,
        Size::Grow(_) => true,
        Size::Auto => {
            inst.node.lays_out_children()
                && inst.children.iter().any(|&g| {
                    tree.get(g).layout.abs.is_none() && grower_inside(tree, g, horizontal)
                })
        }
    }
}

fn text_shrinkable(tree: &InstTree<'_>, c: u32) -> bool {
    let inst = tree.get(c);
    if matches!(inst.layout.w, Size::Px(_)) {
        return false;
    }
    match inst.node.kind {
        NodeKind::Label { wrap: false, .. } | NodeKind::Badge { .. } => {
            inst.node.bind.text.is_some()
        }
        _ if inst.node.lays_out_children() => inst
            .children
            .iter()
            .any(|&g| tree.get(g).layout.abs.is_none() && text_shrinkable(tree, g)),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
