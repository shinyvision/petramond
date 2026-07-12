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
        NodeKind::List => {
            if node.children.len() != 1 {
                issue(format!(
                    "list needs exactly one template child, has {}",
                    node.children.len()
                ));
            }
            if node.bind.items.is_none() {
                issue("list needs an 'items' binding".into());
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
        NodeKind::Image { image, .. } => {
            if image.is_empty() && node.bind.image.is_none() {
                issue("image needs a name or an 'image' binding".into());
            }
        }
        NodeKind::Button { icon, .. } => {
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
}
