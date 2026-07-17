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
mod tests;
