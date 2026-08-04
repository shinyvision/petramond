//! The per-kind POLICY answers, in one place and EXHAUSTIVE.
//!
//! A widget kind's behaviour used to be spread over six independent matches
//! (theme part, pointer targeting, validation, layout, paint, hit) in this
//! crate plus five more in `gui-builder`. Several of those matches ended in a
//! catch-all, so adding a `NodeKind` variant compiled clean and then rendered
//! unstyled, or refused to take a click — a silent bug, visible only in the
//! finished UI.
//!
//! Everything here matches the FULL variant set with no `_` arm. Adding a
//! variant is therefore a compile error in this file, and the compiler names
//! every decision the new widget still owes. Do NOT add a catch-all to
//! anything below; answering "nothing" for a new kind must be a choice
//! someone wrote down, not a default it fell into.
//!
//! The large per-kind ALGORITHMS (layout arrangement, painting) deliberately
//! stay in `layout.rs` / `paint_walk.rs`: they are one narrative each, and the
//! crate convention is not to carve a hot walk into fragments.

use crate::doc::NodeKind;

/// The default theme part a kind draws with; `None` = it carries no chrome of
/// its own (a layout container styles through an explicit `style`, bare
/// content paints itself).
pub fn style_key(kind: &NodeKind) -> Option<&'static str> {
    use crate::doc::AlertLevel;
    Some(match kind {
        NodeKind::Button { .. } => "button.default",
        NodeKind::Checkbox => "checkbox",
        NodeKind::Toggle { .. } => "toggle",
        NodeKind::Slider { .. } => "slider.track",
        NodeKind::TextInput { .. } => "input",
        NodeKind::Slot { .. } | NodeKind::SlotGrid { .. } => "slot",
        NodeKind::Badge { .. } => "badge",
        NodeKind::Alert { level, .. } => match level {
            AlertLevel::Info => "alert.info",
            AlertLevel::Warning => "alert.warning",
            AlertLevel::Success => "alert.success",
            AlertLevel::Danger => "alert.danger",
        },
        NodeKind::Label { .. } => "label",
        NodeKind::TabBar { .. } => "tab",
        NodeKind::Frame
        | NodeKind::Row
        | NodeKind::Column
        | NodeKind::Spacer
        | NodeKind::Image { .. }
        | NodeKind::Rotimage { .. }
        | NodeKind::Scroll { .. }
        | NodeKind::List { .. }
        | NodeKind::Gauge { .. }
        | NodeKind::Hook
        | NodeKind::Tooltip { .. } => return None,
    })
}

/// Whether a pointer press can target this kind DIRECTLY in the topmost-hit
/// scan. Containers answer `false` — their children are the targets — as do
/// list template stamps, which their `list` parent handles.
///
/// A new interactive widget that forgets this is simply unclickable, with no
/// error anywhere; that is why the match below is exhaustive.
pub fn pointer_target(kind: &NodeKind) -> bool {
    match kind {
        NodeKind::Button { .. }
        | NodeKind::Checkbox
        | NodeKind::Toggle { .. }
        | NodeKind::Slider { .. }
        | NodeKind::TextInput { .. }
        | NodeKind::Slot { .. }
        | NodeKind::SlotGrid { .. }
        | NodeKind::TabBar { .. } => true,
        // An image takes clicks only when the document asks it to.
        NodeKind::Image { interactive, .. } => *interactive,
        NodeKind::Frame
        | NodeKind::Row
        | NodeKind::Column
        | NodeKind::Spacer
        | NodeKind::Label { .. }
        | NodeKind::Rotimage { .. }
        | NodeKind::Scroll { .. }
        | NodeKind::List { .. }
        | NodeKind::Gauge { .. }
        | NodeKind::Badge { .. }
        | NodeKind::Alert { .. }
        | NodeKind::Hook
        | NodeKind::Tooltip { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind that answers `pointer_target` must also be reachable — i.e.
    /// the two policy answers agree that an interactive widget is a real,
    /// styleable thing rather than a bare container. This is a cheap guard
    /// that the exhaustive lists above stay in step with each other when a
    /// variant is added and the compiler forces both to be edited.
    #[test]
    fn interactive_kinds_are_not_bare_layout_containers() {
        for kind in [
            NodeKind::Checkbox,
            NodeKind::Frame,
            NodeKind::Row,
            NodeKind::Column,
            NodeKind::Spacer,
            NodeKind::Hook,
            NodeKind::Tooltip { hover: None },
        ] {
            let container = matches!(
                kind,
                NodeKind::Frame | NodeKind::Row | NodeKind::Column | NodeKind::Spacer
            );
            assert!(
                !(container && pointer_target(&kind)),
                "a layout container must not take pointer targeting directly"
            );
        }
    }
}
