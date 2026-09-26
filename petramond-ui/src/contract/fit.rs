//! The viewport-fit rule: a document must lay out inside the smallest
//! viewport the game scales to.
//!
//! The font's line box drives every document's vertical budget, so a font
//! swap (or one more label) must not push a screen off that viewport — seen
//! in game as the Back button sliced off the bottom edge, where nothing can
//! click it. Every instance is judged against ITS PARENT's content box, not
//! the root: the root is the viewport by construction, and parent-relative
//! also catches a tab page that outgrows its panel and paints straight
//! through the buttons below it.
//!
//! Exempt: content inside a `scroll` (overflowing vertically is what it is
//! for), floating tooltip subtrees (the runtime places and clamps them),
//! `abs` children (deliberately out of flow) and zero-height instances.

use crate::doc::{Document, Node, NodeKind};
use crate::layout::{solve, LayoutEnv, RectI};
use crate::state::UiState;
use crate::theme::{Theme, ThemeEnv};
use crate::tree::{InstTree, ROOT};
use crate::validate::DocIssue;

/// The tightest logical viewport any GUI scale solves into: the scale steps
/// up only once the window holds another whole 320×240, so every scale
/// bottoms out at this box.
pub const SMALLEST_VIEWPORT: (i32, i32) = (320, 240);

/// Every instance of `doc`, expanded against `state` and solved at `scale`
/// into [`SMALLEST_VIEWPORT`], that leaves its parent's content box. Issue
/// paths use the validator's `root/2/0(type#id)` form.
pub fn viewport_overflow(
    doc: &Document,
    theme: &Theme,
    state: &UiState,
    scale: i32,
    image_size: &dyn Fn(&str) -> Option<(i32, i32)>,
) -> Vec<DocIssue> {
    let viewport = SMALLEST_VIEWPORT;
    // The responsive breakpoint resolves exactly as the runtime does: a
    // document that stacks below its threshold is judged in that form.
    let tree = InstTree::expand_form(doc, state, doc.compact_active(viewport.0));
    let env = ThemeEnv {
        theme,
        gui_scale: scale,
        image_size,
    };
    let solved = solve(&tree, &env, viewport, &|_| 0);
    // Parents precede children in the arena, so one forward pass inherits
    // the scroll exemption.
    let mut scrolled = vec![false; tree.len()];
    let mut issues = Vec::new();
    for (i, inst) in tree.insts.iter().enumerate() {
        let Some(p) = inst.parent else { continue };
        scrolled[i] =
            scrolled[p as usize] || matches!(tree.get(p).node.kind, NodeKind::Scroll { .. });
        let rect = solved.rects[i];
        if solved.overlay[i] || rect.h == 0 || inst.layout.abs.is_some() {
            continue;
        }
        let parent = tree.get(p);
        let pad = parent.layout.pad;
        let border = env.container_insets(parent.node);
        let content = solved.rects[p as usize].inset(std::array::from_fn(|k| pad[k] + border[k]));
        let (top, bottom) = (content.y, content.y + content.h);
        if (!scrolled[i] && (rect.y < top || rect.y + rect.h > bottom))
            || rect.x < content.x
            || rect.x + rect.w > content.x + content.w
        {
            issues.push(DocIssue {
                path: inst_path(&tree, i as u32),
                message: format!(
                    "at gui scale {scale} lays out at {} outside its parent's content box {} \
                     in the smallest {}x{} viewport",
                    fmt_rect(rect),
                    fmt_rect(content),
                    viewport.0,
                    viewport.1
                ),
            });
        }
    }
    issues
}

/// The document path of instance `i` (`root/2/0(button#go)`), found by
/// walking parents: each step is the node's index among its parent node's
/// children, so list-stamped copies share their template's path.
fn inst_path(tree: &InstTree<'_>, i: u32) -> String {
    let mut steps = Vec::new();
    let mut at = i;
    while let Some(p) = tree.get(at).parent {
        let child: &Node = tree.get(at).node;
        let parent = tree.get(p).node;
        if let Some(idx) = parent.children.iter().position(|c| std::ptr::eq(c, child)) {
            steps.push(idx);
        }
        at = p;
    }
    debug_assert_eq!(at, ROOT);
    let mut path = String::from("root");
    for idx in steps.iter().rev() {
        path.push('/');
        path.push_str(&idx.to_string());
    }
    super::node_label(&path, tree.get(i).node)
}

fn fmt_rect(r: RectI) -> String {
    format!("{}x{}@({},{})", r.w, r.h, r.x, r.y)
}
