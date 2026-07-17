//! The one-call-per-frame runtime facade: expand → solve → interact → paint.
//!
//! The game and the builder preview both drive a [`UiRuntime`]; everything a
//! frame produces comes back in [`FrameOutput`] — the draw list, resolved
//! widget events, and the named/slot rects the host needs to layer its own
//! content (item icons, hearts) and to hit-test latched clicks.

use crate::doc::{Document, NodeKind};
use crate::input::{FrameState, InputEvent, PreviewState, UiEvent};
use crate::interact::{collect_slots, Interact};
use crate::layout::{grid_cell, solve, RectI};
use crate::paint::{DrawList, Painter, TexId, SOLID_UV};
use crate::paint_walk::{DocImages, PaintCtx};
use crate::text_edit::TextClipboard;
use crate::theme::{Theme, ThemeEnv};
use crate::tree::{InstKey, InstTree, ROOT};
use crate::widget;
use std::sync::Arc;

pub struct UiRuntime {
    doc: Arc<Document>,
    theme: Arc<Theme>,
}

pub struct FrameArgs<'a> {
    /// Physical framebuffer size, px.
    pub screen: (u32, u32),
    /// Integer gui scale (logical px × scale = physical px).
    pub scale: i32,
    /// Host time in seconds (blink, double-click windows).
    pub now: f64,
    pub state: &'a crate::state::UiState,
    /// Input events since the last frame, in order.
    pub input: &'a [InputEvent],
    pub clipboard: Option<&'a mut dyn TextClipboard>,
    pub images: &'a dyn DocImages,
    /// Fullscreen backdrop color behind the GUI (menus dim the world).
    pub dim: Option<[f32; 4]>,
    /// Builder-only forced widget states.
    pub preview: Option<&'a PreviewState>,
}

/// One slot cell's physical rect, by role + in-role index.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotRectOut {
    pub role: String,
    pub index: u32,
    pub rect: RectI,
}

/// One host-drawn `hook` instance. The rect and inherited clip are physical
/// pixels, matching [`SlotRectOut`]; `key.item` identifies a repeated list
/// row. Hosts must apply `clip` when drawing content inside scrolling hooks.
#[derive(Clone, Debug, PartialEq)]
pub struct HookRectOut {
    pub key: InstKey,
    pub rect: RectI,
    pub clip: Option<RectI>,
}

#[derive(Default)]
pub struct FrameOutput {
    pub draw: DrawList,
    pub events: Vec<UiEvent>,
    /// Physical rects of every id-bearing instance (hooks, widgets).
    pub named: Vec<(InstKey, RectI)>,
    /// Physical rects and inherited clips for host-drawn `hook` instances.
    pub hooks: Vec<HookRectOut>,
    /// Physical rects of every slot cell, role-indexed like the host's slots.
    pub slots: Vec<SlotRectOut>,
    /// The root panel's physical rect (outside = cursor-throw territory).
    pub panel_rect: RectI,
    /// The slot cell under the cursor, if any.
    pub hover_slot: Option<(String, u32)>,
}

impl FrameOutput {
    /// The physical rect of instance `id` (first match).
    pub fn rect(&self, id: &str) -> Option<RectI> {
        self.named.iter().find(|(k, _)| k.id == id).map(|(_, r)| *r)
    }
}

impl UiRuntime {
    pub fn new(doc: Arc<Document>, theme: Arc<Theme>) -> UiRuntime {
        UiRuntime { doc, theme }
    }

    pub fn doc(&self) -> &Arc<Document> {
        &self.doc
    }

    pub fn theme(&self) -> &Arc<Theme> {
        &self.theme
    }

    pub fn frame(&self, mut args: FrameArgs<'_>, fs: &mut FrameState, out: &mut FrameOutput) {
        out.draw.clear();
        out.events.clear();
        out.named.clear();
        out.hooks.clear();
        out.slots.clear();
        out.hover_slot = None;
        out.panel_rect = RectI::ZERO;
        if args.screen.0 == 0 || args.screen.1 == 0 || args.scale <= 0 {
            return;
        }
        fs.now = args.now;
        // The click bridge lives exactly one frame (set below in interaction,
        // read by this frame's paint).
        fs.clicked = None;

        let scale = args.scale;
        let viewport = (
            (args.screen.0 as i32) / scale,
            (args.screen.1 as i32) / scale,
        );
        let tree = InstTree::expand_form(&self.doc, args.state, self.doc.compact_active(viewport.0));
        if tree.is_empty() {
            return;
        }
        let images = args.images;
        let env = ThemeEnv {
            theme: &self.theme,
            image_size: &|name| images.resolve(name).map(|(_, (w, h))| (w as i32, h as i32)),
        };
        let solved = solve(&tree, &env, viewport, &|i| {
            tree.get(i)
                .key
                .as_ref()
                .map(|k| fs.scroll_offset(k))
                .unwrap_or(0)
        });

        // Re-clamp scroll offsets against this frame's content so a shrunk
        // list can't strand its offset out of range.
        for i in 0..tree.len() as u32 {
            let inst = tree.get(i);
            let NodeKind::Scroll { axis } = inst.node.kind else {
                continue;
            };
            let Some(key) = inst.key.clone() else {
                continue;
            };
            let (viewport_len, content_len) = widget::scroll_lengths(
                axis,
                solved.rects[i as usize],
                solved.scroll_content[i as usize].unwrap_or((0, 0)),
            );
            let clamped = widget::clamp_scroll(fs.scroll_offset(&key), viewport_len, content_len);
            if clamped != fs.scroll_offset(&key) {
                fs.set_scroll(key, clamped);
            }
        }

        // A list whose bound selection CHANGED (keyboard nav) scrolls its
        // enclosing scroll region to keep the selected row visible.
        for i in 0..tree.len() as u32 {
            let inst = tree.get(i);
            if !matches!(inst.node.kind, NodeKind::List) {
                continue;
            }
            let (Some(key), Some(selected)) = (inst.key.clone(), inst.selected) else {
                continue;
            };
            let changed = fs.last_selected.get(&key) != Some(&selected);
            fs.last_selected.insert(key, selected);
            if !changed || selected < 0 {
                continue;
            }
            let Some(&row_inst) = inst.children.get(selected as usize) else {
                continue;
            };
            // The nearest scroll ancestor owns the offset.
            let mut anc = inst.parent;
            while let Some(a) = anc {
                if matches!(tree.get(a).node.kind, NodeKind::Scroll { .. }) {
                    break;
                }
                anc = tree.get(a).parent;
            }
            let Some(scroll_i) = anc else { continue };
            let Some(scroll_key) = tree.get(scroll_i).key.clone() else {
                continue;
            };
            let view = solved.rects[scroll_i as usize];
            let row = solved.rects[row_inst as usize];
            let off = fs.scroll_offset(&scroll_key);
            let new_off = if row.y < view.y {
                off - (view.y - row.y)
            } else if row.y + row.h > view.y + view.h {
                off + (row.y + row.h - view.y - view.h)
            } else {
                off
            };
            let content = solved.scroll_content[scroll_i as usize].unwrap_or((0, 0));
            let new_off = widget::clamp_scroll(new_off, view.h, content.1);
            if new_off != off {
                fs.set_scroll(scroll_key, new_off);
            }
        }

        let slots = collect_slots(&tree);
        let metrics = crate::layout::SlotMetrics {
            slot: self.theme.metrics.slot,
            gap: self.theme.metrics.slot_gap,
        };
        let interact = Interact {
            tree: &tree,
            solved: &solved,
            theme: &self.theme,
            scale,
            slots: &slots,
            metrics,
        };
        interact.run(fs, args.input, args.clipboard.take(), &mut out.events);

        // Hover resolution for paint, from the post-input cursor.
        let (cx, cy) = (fs.cursor().0 / scale as f32, fs.cursor().1 / scale as f32);
        let visible_at = |i: u32| {
            widget::contains_f(solved.rects[i as usize], cx, cy)
                && solved.clips[i as usize].is_none_or(|c| widget::contains_f(c, cx, cy))
        };
        let hover = (0..tree.len() as u32)
            .rev()
            .find(|&i| tree.get(i).enabled && widget::pointer_target(tree.get(i)) && visible_at(i));
        let slot_hover = hover.and_then(|i| match tree.get(i).node.kind {
            NodeKind::Slot { .. } => Some((i, 0)),
            NodeKind::SlotGrid { cols, rows, .. } => (0..cols * rows)
                .find(|&c| {
                    widget::contains_f(
                        grid_cell(solved.rects[i as usize], cols, c, metrics),
                        cx,
                        cy,
                    )
                })
                .map(|c| (i, c)),
            _ => None,
        });
        let row_hover = (0..tree.len() as u32).rev().find_map(|i| {
            if !matches!(tree.get(i).node.kind, NodeKind::List) || !visible_at(i) {
                return None;
            }
            tree.get(i)
                .children
                .iter()
                .position(|&c| tree.get(c).enabled && visible_at(c))
                .map(|row| (i, row as u32))
        });
        let tab_hover = hover.and_then(|i| match &tree.get(i).node.kind {
            NodeKind::TabBar { tabs } => {
                let widths = widget::tab_widths(&self.theme, tabs);
                widget::tab_hit(
                    solved.rects[i as usize],
                    &widths,
                    self.theme.metrics.tab_gap,
                    cx,
                    cy,
                )
                .map(|t| (i, t))
            }
            _ => None,
        });

        // Paint: dim backdrop (physical fullscreen), then the tree.
        if let Some(color) = args.dim {
            let (w, h) = (args.screen.0 as f32, args.screen.1 as f32);
            out.draw.push_quad(
                TexId::Solid,
                [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
                [SOLID_UV; 4],
                color,
                None,
            );
        }
        let ctx = PaintCtx {
            tree: &tree,
            solved: &solved,
            theme: &self.theme,
            fs,
            images,
            metrics,
            hover,
            slot_hover,
            row_hover,
            tab_hover,
            preview: args.preview,
        };
        ctx.paint(&mut Painter {
            list: &mut out.draw,
            scale,
        });

        // Outputs the host layers content with, all in physical px.
        let phys = |r: RectI| RectI {
            x: r.x * scale,
            y: r.y * scale,
            w: r.w * scale,
            h: r.h * scale,
        };
        out.panel_rect = phys(solved.rects[ROOT as usize]);
        for (i, inst) in tree.insts.iter().enumerate() {
            if let Some(key) = &inst.key {
                let rect = phys(solved.rects[i]);
                out.named.push((key.clone(), rect));
                if matches!(inst.node.kind, NodeKind::Hook) {
                    out.hooks.push(HookRectOut {
                        key: key.clone(),
                        rect,
                        clip: solved.clips[i].map(phys),
                    });
                }
            }
        }
        for slot in &slots {
            let rect = solved.rects[slot.inst as usize];
            let (cols, cells) = match tree.get(slot.inst).node.kind {
                NodeKind::SlotGrid { cols, rows, .. } => (cols, cols * rows),
                _ => (1, 1),
            };
            for c in 0..cells {
                out.slots.push(SlotRectOut {
                    role: slot.role.clone(),
                    index: slot.base + c,
                    rect: phys(grid_cell(rect, cols, c, metrics)),
                });
            }
        }
        out.hover_slot = slot_hover.and_then(|(i, c)| {
            slots
                .iter()
                .find(|s| s.inst == i)
                .map(|s| (s.role.clone(), s.base + c))
        });
    }
}

#[cfg(test)]
mod tests;
