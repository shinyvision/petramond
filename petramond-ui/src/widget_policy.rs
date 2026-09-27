use crate::doc::NodeKind;

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
        | NodeKind::Canvas { .. }
        | NodeKind::Viewport { .. }
        | NodeKind::Tooltip { .. } => return None,
    })
}

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
        NodeKind::Image { interactive, .. }
        | NodeKind::Canvas { interactive }
        | NodeKind::Viewport { interactive } => *interactive,
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
