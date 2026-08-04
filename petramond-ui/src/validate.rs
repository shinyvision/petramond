//! Document validation: the load-time contract that keeps a bad document from
//! ever mis-routing a click or silently dropping content.
//!
//! Structural rules live here (ids, arity, bindings); the *host* supplies the
//! per-kind [`SlotContract`] (which roles, how many) and a [`StyleLookup`]
//! (which theme parts exist). A document that validates against its contract
//! can never mis-map an in-role index: grids generate cells row-major and the
//! contract pins the counts.

use crate::doc::{Document, Node, NodeKind};
use std::collections::HashSet;

/// The host's slot expectations for one document kind: every role it must
/// declare with exact counts. Roles absent from the contract are forbidden —
/// an empty contract means "no slots at all" (mod GUI kinds).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlotContract {
    pub roles: Vec<(String, usize)>,
}

impl SlotContract {
    pub fn new(roles: &[(&str, usize)]) -> SlotContract {
        SlotContract {
            roles: roles.iter().map(|(r, n)| ((*r).to_owned(), *n)).collect(),
        }
    }
}

/// Something that knows which theme part keys exist (implemented by `Theme`).
pub trait StyleLookup {
    fn has_style(&self, key: &str) -> bool;
}

/// One validation finding, anchored by a node path like
/// `root/2/0(button#spin)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocIssue {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for DocIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl Document {
    /// Every violated rule (empty = valid). `styles`/`contract` are optional
    /// so structural checks run without a theme (builder while art is WIP)
    /// or before the host resolves the kind.
    pub fn validate(
        &self,
        styles: Option<&dyn StyleLookup>,
        contract: Option<&SlotContract>,
    ) -> Vec<DocIssue> {
        let mut issues = Vec::new();
        if self.kind.is_empty() {
            issues.push(DocIssue {
                path: "document".into(),
                message: "kind key is empty".into(),
            });
        }
        if let Some(w) = self.compact_below_w {
            if w <= 0 {
                issues.push(DocIssue {
                    path: "document".into(),
                    message: format!("compact_below_w must be positive, got {w}"),
                });
            }
        }
        let mut seen_ids: HashSet<&str> = HashSet::new();
        walk(&self.root, "root", &mut seen_ids, styles, &mut issues);
        // Hover anchors resolve against the WHOLE id set, so they get their
        // own pass: a tooltip may sit earlier in document order than the
        // widget it names.
        check_hover_anchors(&self.root, "root", &seen_ids, &mut issues);

        if let Some(contract) = contract {
            let declared = self.role_slots();
            for (role, want) in &contract.roles {
                match declared.iter().find(|(r, _)| r == role) {
                    Some((_, got)) if got == want => {}
                    Some((_, got)) => issues.push(DocIssue {
                        path: "document".into(),
                        message: format!(
                            "role '{role}' declares {got} slots, contract wants {want}"
                        ),
                    }),
                    None => issues.push(DocIssue {
                        path: "document".into(),
                        message: format!("role '{role}' missing ({want} slots required)"),
                    }),
                }
            }
            for (role, _) in &declared {
                if !contract.roles.iter().any(|(r, _)| r == role) {
                    issues.push(DocIssue {
                        path: "document".into(),
                        message: format!("role '{role}' is not in this kind's contract"),
                    });
                }
            }
        }
        issues
    }
}

/// Sprite-sheet grid rule shared by `image` and image-backed `button`:
/// both dimensions must be non-zero.
fn check_frames(frames: &Option<[u32; 2]>, issue: &mut impl FnMut(String)) {
    if let Some([cols, rows]) = frames {
        if *cols == 0 || *rows == 0 {
            issue(format!("frames grid must be >= 1x1, got {cols}x{rows}"));
        }
    }
}

/// Animation-rate rule: when `fps` is present it must be positive and finite
/// (anything else would silently rest on frame 0).
fn check_fps(fps: &Option<f32>, issue: &mut impl FnMut(String)) {
    if let Some(fps) = fps {
        if !fps.is_finite() || *fps <= 0.0 {
            issue(format!("fps must be positive and finite, got {fps}"));
        }
    }
}

/// A tooltip's `hover` anchor must name a widget that exists — a typo'd id
/// would not error anywhere else, the tooltip would just never show.
fn check_hover_anchors(
    node: &Node,
    path: &str,
    ids: &HashSet<&str>,
    issues: &mut Vec<DocIssue>,
) {
    if let NodeKind::Tooltip {
        hover: Some(anchor),
    } = &node.kind
    {
        if !ids.contains(anchor.as_str()) {
            issues.push(DocIssue {
                path: path.into(),
                message: format!("tooltip hover anchor '{anchor}' names no widget id"),
            });
        }
    }
    for (i, child) in node.children.iter().enumerate() {
        check_hover_anchors(child, &format!("{path}/{i}"), ids, issues);
    }
}

fn walk<'a>(
    node: &'a Node,
    path: &str,
    seen_ids: &mut HashSet<&'a str>,
    styles: Option<&dyn StyleLookup>,
    issues: &mut Vec<DocIssue>,
) {
    let label = match (&node.id, node.kind.type_name()) {
        (Some(id), t) => format!("{path}({t}#{id})"),
        (None, t) => format!("{path}({t})"),
    };
    let mut issue = |message: String| {
        issues.push(DocIssue {
            path: label.clone(),
            message,
        })
    };

    match &node.id {
        Some(id) if id.is_empty() => issue("empty id".into()),
        Some(id) => {
            if !seen_ids.insert(id) {
                issue(format!("duplicate id '{id}'"));
            }
        }
        None => {
            if node.kind.needs_id() {
                issue(format!("{} requires an id", node.kind.type_name()));
            }
        }
    }

    if !node.kind.is_container() && !node.children.is_empty() {
        issue(format!("{} cannot have children", node.kind.type_name()));
    }

    match &node.kind {
        NodeKind::List { cols } => {
            if node.children.len() != 1 {
                issue(format!(
                    "list needs exactly one template child, has {}",
                    node.children.len()
                ));
            }
            if node.bind.items.is_none() {
                issue("list needs an 'items' binding".into());
            }
            if *cols == 0 {
                issue("list cols must be >= 1".into());
            }
        }
        NodeKind::Tooltip { .. } => {
            if node.children.is_empty() {
                issue("tooltip needs at least one child".into());
            }
            // Without a visibility bind a tooltip would follow the pointer on
            // every frame of the screen's life.
            if node.bind.visible.is_none() {
                issue("tooltip needs a 'visible' binding (the host shows it)".into());
            }
        }
        NodeKind::Slot { role, .. } | NodeKind::SlotGrid { role, .. } if role.is_empty() => {
            issue("empty slot role".into());
        }
        NodeKind::SlotGrid { cols, rows, .. } => {
            if *cols == 0 || *rows == 0 {
                issue("slot_grid needs cols and rows >= 1".into());
            }
        }
        NodeKind::Gauge { .. } => {
            if node.bind.value.is_none() {
                issue("gauge needs a 'value' binding".into());
            }
        }
        NodeKind::Rotimage { image, .. } => {
            if node.bind.value.is_none() {
                issue("rotimage needs a 'value' binding (radians)".into());
            }
            if image.is_empty() && node.bind.image.is_none() {
                issue("rotimage needs an image name or an 'image' binding".into());
            }
        }
        NodeKind::Image {
            image, frames, fps, ..
        } => {
            if image.is_empty() && node.bind.image.is_none() {
                issue("image needs a name or an 'image' binding".into());
            }
            check_frames(frames, &mut issue);
            check_fps(fps, &mut issue);
        }
        NodeKind::Button {
            icon,
            image,
            frames,
            fps,
            ..
        } => {
            let image_backed = image.as_deref().is_some_and(|s| !s.is_empty());
            if !node.children.is_empty()
                && (node.bind.text.is_some()
                    || matches!(&node.kind, NodeKind::Button { text: Some(_), .. })
                    || icon.is_some())
            {
                issue(
                    "button children replace its inline text/icon; remove text, icon, and text binding"
                        .into(),
                );
            }
            if image_backed
                && (!node.children.is_empty()
                    || node.bind.text.is_some()
                    || matches!(&node.kind, NodeKind::Button { text: Some(_), .. })
                    || icon.is_some())
            {
                issue(
                    "an image-backed button carries no text, icon, or children; remove them".into(),
                );
            }
            check_frames(frames, &mut issue);
            check_fps(fps, &mut issue);
            if let (Some(styles), Some(icon)) = (styles, icon.as_deref()) {
                if !styles.has_style(icon) {
                    issue(format!("unknown icon part '{icon}'"));
                }
            }
        }
        NodeKind::Toggle { icon } => {
            if let (Some(styles), Some(icon)) = (styles, icon.as_deref()) {
                if !styles.has_style(icon) {
                    issue(format!("unknown icon part '{icon}'"));
                }
            }
        }
        NodeKind::Label { text, .. } => {
            if text.is_none() && node.bind.text.is_none() {
                issue("label needs static 'text' or a 'text' binding".into());
            }
        }
        NodeKind::TabBar { tabs } => {
            if tabs.is_empty() {
                issue("tab_bar needs at least one tab".into());
            }
            if node.bind.selected.is_none() {
                issue("tab_bar needs a 'selected' binding".into());
            }
            let mut keys: HashSet<&str> = HashSet::new();
            for (i, tab) in tabs.iter().enumerate() {
                if tab.key.is_empty() {
                    issue(format!("tab {i} has an empty key"));
                } else if !keys.insert(tab.key.as_str()) {
                    issue(format!("duplicate tab key '{}'", tab.key));
                }
                if tab.icon.is_none() && tab.label.as_deref().unwrap_or("").is_empty() {
                    issue(format!("tab '{}' needs an icon or a label", tab.key));
                }
                if let (Some(styles), Some(icon)) = (styles, tab.icon.as_deref()) {
                    if !styles.has_style(icon) {
                        issue(format!("unknown icon part '{icon}'"));
                    }
                }
            }
        }
        NodeKind::Slider { min, max, step } => {
            if max <= min {
                issue(format!("slider range is empty ({min}..{max})"));
            }
            if let Some(step) = step {
                if *step <= 0.0 {
                    issue(format!("slider step must be positive, got {step}"));
                }
            }
        }
        _ => {}
    }

    if let (Some(styles), Some(style)) = (styles, &node.style) {
        if !styles.has_style(style) {
            issue(format!("unknown style '{style}'"));
        }
    }

    for (i, child) in node.children.iter().enumerate() {
        walk(child, &format!("{path}/{i}"), seen_ids, styles, issues);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;

    struct Styles(Vec<&'static str>);
    impl StyleLookup for Styles {
        fn has_style(&self, key: &str) -> bool {
            self.0.contains(&key)
        }
    }

    fn doc(json: &str) -> Document {
        Document::from_json(json).unwrap()
    }

    #[test]
    fn valid_container_passes_its_contract() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:chest", "class": "container",
            "root": { "type": "column", "children": [
                { "type": "slot_grid", "role": "storage", "cols": 9, "rows": 3 },
                { "type": "slot_grid", "role": "player_inv", "cols": 9, "rows": 3 },
                { "type": "slot_grid", "role": "hotbar", "cols": 9, "rows": 1 }
            ] }
        }"#);
        let contract = SlotContract::new(&[("storage", 27), ("player_inv", 27), ("hotbar", 9)]);
        assert_eq!(d.validate(None, Some(&contract)), vec![]);
    }

    #[test]
    fn contract_catches_wrong_count_missing_and_foreign_roles() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:chest", "class": "container",
            "root": { "type": "column", "children": [
                { "type": "slot_grid", "role": "storage", "cols": 9, "rows": 2 },
                { "type": "slot", "role": "mystery" }
            ] }
        }"#);
        let contract = SlotContract::new(&[("storage", 27), ("hotbar", 9)]);
        let issues = d.validate(None, Some(&contract));
        let all = issues
            .iter()
            .map(|i| i.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(all.contains("'storage' declares 18"), "{all}");
        assert!(all.contains("'hotbar' missing"), "{all}");
        assert!(
            all.contains("'mystery' is not in this kind's contract"),
            "{all}"
        );
    }

    #[test]
    fn empty_contract_forbids_all_slots() {
        let d = doc(r#"{
            "format": 1, "kind": "somemod:wheel", "class": "container",
            "root": { "type": "frame", "children": [
                { "type": "slot", "role": "hotbar" }
            ] }
        }"#);
        let issues = d.validate(None, Some(&SlotContract::default()));
        assert!(issues
            .iter()
            .any(|i| i.message.contains("not in this kind's contract")));
    }

    #[test]
    fn ids_must_be_present_on_event_widgets_and_unique() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:pause", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "button", "text": "Resume" },
                { "type": "button", "id": "a", "text": "X" },
                { "type": "toggle", "id": "a" }
            ] }
        }"#);
        let issues = d.validate(None, None);
        let all = issues
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(all.contains("button requires an id"), "{all}");
        assert!(all.contains("duplicate id 'a'"), "{all}");
    }

    #[test]
    fn structural_widget_rules() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "list", "id": "l", "bind": { "items": "rows" } },
                { "type": "gauge", "mode": "grow_lr" },
                { "type": "rotimage", "image": "wheel.png" },
                { "type": "label" },
                { "type": "slider", "id": "s", "min": 5.0, "max": 5.0 },
                { "type": "label", "text": "x", "children": [ { "type": "spacer" } ] }
            ] }
        }"#);
        let issues = d.validate(None, None);
        let all = issues
            .iter()
            .map(|i| i.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(all.contains("exactly one template child"), "{all}");
        assert!(all.contains("gauge needs a 'value' binding"), "{all}");
        assert!(all.contains("rotimage needs a 'value' binding"), "{all}");
        assert!(all.contains("label needs static 'text'"), "{all}");
        assert!(all.contains("slider range is empty"), "{all}");
        assert!(all.contains("label cannot have children"), "{all}");
    }

    #[test]
    fn style_refs_check_against_the_lookup() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame", "style": "panel.large", "children": [
                { "type": "button", "id": "b", "text": "OK", "style": "button.bogus" }
            ] }
        }"#);
        let styles = Styles(vec!["panel.large", "button.default"]);
        let issues = d.validate(Some(&styles), None);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("unknown style 'button.bogus'"));
        assert!(issues[0].path.contains("button#b"));
    }

    #[test]
    fn compound_button_accepts_children_but_not_overlapping_inline_content() {
        let compound = doc(r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "button", "id": "recipe", "children": [
                { "type": "label", "text": "Recipe" },
                { "type": "hook", "id": "icon", "layout": { "w": 16, "h": 16 } }
            ] }
        }"#);
        assert!(compound.validate(None, None).is_empty());

        let overlapping = doc(r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "button", "id": "recipe", "text": "Inline",
                "children": [ { "type": "label", "text": "Child" } ] }
        }"#);
        assert!(overlapping.validate(None, None).iter().any(|issue| issue
            .message
            .contains("children replace its inline text/icon")));
    }

    #[test]
    fn framed_nodes_validate_grid_fps_and_image_button_content() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "image", "image": "a.png", "frames": [0, 2] },
                { "type": "image", "image": "b.png", "fps": 0.0 },
                { "type": "image", "image": "c.png", "fps": -2.0 },
                { "type": "button", "id": "i1", "image": "d.png", "text": "Hi" },
                { "type": "button", "id": "i2", "image": "d.png", "frames": [3, 0] },
                { "type": "button", "id": "i3", "image": "d.png",
                  "children": [ { "type": "label", "text": "x" } ] }
            ] }
        }"#);
        let issues = d.validate(None, None);
        let all = issues
            .iter()
            .map(|i| i.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let count = |needle: &str| issues.iter().filter(|i| i.message.contains(needle)).count();
        assert_eq!(count("frames grid must be >= 1x1"), 2, "{all}");
        assert_eq!(count("fps must be positive and finite"), 2, "{all}");
        assert_eq!(
            count("an image-backed button carries no text, icon, or children"),
            2,
            "{all}"
        );
    }

    #[test]
    fn framed_nodes_with_valid_animation_pass() {
        let d = doc(r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "image", "image": "flame.png", "frames": [4, 1], "fps": 8.0,
                  "bind": { "frame": "flame_frame" } },
                { "type": "button", "id": "go", "image": "go.png", "frames": [2, 2], "fps": 4.0 }
            ] }
        }"#);
        assert_eq!(d.validate(None, None), vec![]);
    }
}
