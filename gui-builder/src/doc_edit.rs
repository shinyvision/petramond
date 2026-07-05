//! Pure document-tree editing helpers: node paths (child-index chains from the
//! root), structural mutations, id generation, and the DocIssue path resolver.
//! No egui in here — everything is unit-testable.

use llama_ui::{Document, LayoutProps, Node, NodeKind};

/// A node path: child indices from the root. `[]` is the root itself.
pub type NodePath = Vec<usize>;

pub fn node_at<'a>(root: &'a Node, path: &[usize]) -> Option<&'a Node> {
    let mut n = root;
    for &i in path {
        n = n.children.get(i)?;
    }
    Some(n)
}

pub fn node_at_mut<'a>(root: &'a mut Node, path: &[usize]) -> Option<&'a mut Node> {
    let mut n = root;
    for &i in path {
        n = n.children.get_mut(i)?;
    }
    Some(n)
}

/// Remove the node at `path` (never the root). Returns it.
pub fn remove_at(root: &mut Node, path: &[usize]) -> Option<Node> {
    let (&last, parent) = path.split_last()?;
    let p = node_at_mut(root, parent)?;
    if last < p.children.len() {
        Some(p.children.remove(last))
    } else {
        None
    }
}

/// Insert `node` as child `index` of the container at `parent` (index clamped).
pub fn insert_at(root: &mut Node, parent: &[usize], index: usize, node: Node) -> Option<NodePath> {
    let p = node_at_mut(root, parent)?;
    let i = index.min(p.children.len());
    p.children.insert(i, node);
    let mut path = parent.to_vec();
    path.push(i);
    Some(path)
}

/// Whether `descendant` is `ancestor` or lives inside it.
pub fn is_same_or_descendant(ancestor: &[usize], descendant: &[usize]) -> bool {
    descendant.len() >= ancestor.len() && descendant[..ancestor.len()] == *ancestor
}

/// Move the node at `from` to become child `index` of `to_parent`. Refuses
/// moves into the node's own subtree. Returns the node's new path.
pub fn move_node(root: &mut Node, from: &[usize], to_parent: &[usize], index: usize) -> Option<NodePath> {
    if from.is_empty() || is_same_or_descendant(from, to_parent) {
        return None;
    }
    let node = remove_at(root, from)?;
    // Removing `from` may shift the target parent path / index.
    let mut parent = to_parent.to_vec();
    let mut index = index;
    let (&from_last, from_parent) = from.split_last().unwrap();
    if parent.len() > from_parent.len()
        && parent[..from_parent.len()] == *from_parent
        && parent[from_parent.len()] > from_last
    {
        parent[from_parent.len()] -= 1;
    } else if parent == from_parent && index > from_last {
        index -= 1;
    }
    match insert_at(root, &parent, index, node) {
        Some(p) => Some(p),
        None => {
            // Shouldn't happen; avoid losing the node if it does.
            None
        }
    }
}

/// Every id used anywhere in the document.
pub fn all_ids(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    doc.root.visit(&mut |n| {
        if let Some(id) = &n.id {
            out.push(id.clone());
        }
    });
    out
}

/// A fresh id `base`, `base2`, `base3`… not used in the document.
pub fn unique_id(doc: &Document, base: &str) -> String {
    let used = all_ids(doc);
    if !used.iter().any(|i| i == base) {
        return base.to_owned();
    }
    for n in 2.. {
        let candidate = format!("{base}{n}");
        if !used.iter().any(|i| i == &candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Reassign fresh unique ids to every id-bearing node in `node` (duplication,
/// preset insertion) so the document keeps its ids unique.
pub fn uniquify_ids(doc: &Document, node: &mut Node) {
    let mut used = all_ids(doc);
    fn walk(n: &mut Node, used: &mut Vec<String>) {
        if let Some(id) = &n.id {
            let base: String = id.trim_end_matches(|c: char| c.is_ascii_digit()).to_owned();
            let base = if base.is_empty() { id.clone() } else { base };
            let fresh = if !used.iter().any(|u| u == id) {
                id.clone()
            } else {
                (2..)
                    .map(|k| format!("{base}{k}"))
                    .find(|c| !used.iter().any(|u| u == c))
                    .unwrap()
            };
            used.push(fresh.clone());
            n.id = Some(fresh);
        }
        for c in &mut n.children {
            walk(c, used);
        }
    }
    walk(node, &mut used);
}

/// A new default node of each kind, with an id where the kind requires one.
pub fn new_node(doc: &Document, type_name: &str) -> Option<Node> {
    let kind = match type_name {
        "frame" => NodeKind::Frame,
        "row" => NodeKind::Row,
        "column" => NodeKind::Column,
        "spacer" => NodeKind::Spacer,
        "label" => NodeKind::Label { text: Some("Label".into()), wrap: false, scale: 1 },
        "image" => NodeKind::Image { image: "image.png".into(), fit: Default::default() },
        "rotimage" => NodeKind::Rotimage { image: "image.png".into(), pivot: None },
        "button" => NodeKind::Button { text: Some("BUTTON".into()), icon: None },
        "checkbox" => NodeKind::Checkbox,
        "toggle" => NodeKind::Toggle,
        "slider" => NodeKind::Slider { min: 0.0, max: 100.0, step: None },
        "text_input" => NodeKind::TextInput { placeholder: None, max_chars: 64 },
        "scroll" => NodeKind::Scroll { axis: llama_ui::ScrollAxis::Vertical },
        "list" => NodeKind::List,
        "slot" => NodeKind::Slot { role: "storage".into() },
        "slot_grid" => NodeKind::SlotGrid { role: "storage".into(), cols: 9, rows: 3 },
        "gauge" => NodeKind::Gauge { mode: llama_ui::GaugeMode::GrowLr },
        "badge" => NodeKind::Badge { text: Some("badge".into()) },
        "alert" => NodeKind::Alert { level: llama_ui::AlertLevel::Info, text: Some("Alert text".into()) },
        "hook" => NodeKind::Hook,
        _ => return None,
    };
    let mut node = Node::leaf(kind);
    if node.kind.needs_id() {
        node.id = Some(unique_id(doc, type_name));
    }
    if let NodeKind::Gauge { .. } | NodeKind::Rotimage { .. } = node.kind {
        node.bind.value = Some("value".into());
    }
    if let NodeKind::List = node.kind {
        node.bind.items = Some("items".into());
        node.children.push(Node::leaf(NodeKind::Label {
            text: Some("Row".into()),
            wrap: false,
            scale: 1,
        }));
    }
    Some(node)
}

/// Every insertable node type name, palette order.
pub const NODE_TYPES: &[&str] = &[
    "frame", "row", "column", "spacer", "label", "image", "rotimage", "button", "checkbox",
    "toggle", "slider", "text_input", "scroll", "list", "slot", "slot_grid", "gauge", "badge",
    "alert", "hook",
];

/// Wrap the node at `path` in a fresh row/column container in place.
pub fn wrap_in(root: &mut Node, path: &[usize], kind: NodeKind) -> Option<()> {
    if path.is_empty() {
        return None;
    }
    let node = remove_at(root, path)?;
    let mut wrapper = Node::leaf(kind);
    wrapper.layout = LayoutProps::default();
    wrapper.children.push(node);
    insert_at(root, &path[..path.len() - 1], *path.last().unwrap(), wrapper)?;
    Some(())
}

// ---- static images ----------------------------------------------------------

/// Every static image file name the document references (`image`/`rotimage`
/// nodes), deduplicated in document order. Bound image names (`bind.image`)
/// are state keys, not files, so they don't count.
pub fn static_image_names(doc: &Document) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    doc.root.visit(&mut |n| {
        if let NodeKind::Image { image, .. } | NodeKind::Rotimage { image, .. } = &n.kind {
            if !out.contains(image) {
                out.push(image.clone());
            }
        }
    });
    out
}

/// Builder-side warnings for static image names that don't resolve (the node
/// draws nothing). Issue paths use the runtime's `root/1/2(image)` format so
/// the validation panel's click-to-select resolver works on them.
pub fn missing_image_issues(
    doc: &Document,
    exists: &dyn Fn(&str) -> bool,
) -> Vec<llama_ui::DocIssue> {
    fn walk(
        node: &Node,
        path: &str,
        exists: &dyn Fn(&str) -> bool,
        out: &mut Vec<llama_ui::DocIssue>,
    ) {
        if let NodeKind::Image { image, .. } | NodeKind::Rotimage { image, .. } = &node.kind {
            if !exists(image) {
                out.push(llama_ui::DocIssue {
                    path: format!("{path}({})", node.kind.type_name()),
                    message: format!(
                        "image '{image}' not found beside the project (won't draw; \
                         use Choose image… to copy one in)"
                    ),
                });
            }
        }
        for (i, c) in node.children.iter().enumerate() {
            walk(c, &format!("{path}/{i}"), exists, out);
        }
    }
    let mut out = Vec::new();
    walk(&doc.root, "root", exists, &mut out);
    out
}

// ---- DocIssue path resolver ---------------------------------------------------

/// Resolve a `DocIssue.path` (e.g. `root/2/0(button#spin)` or `document`) back
/// to a node path. Document-level issues resolve to the root.
pub fn resolve_issue_path(path: &str) -> Option<NodePath> {
    if path == "document" {
        return Some(Vec::new());
    }
    let structural = match path.find('(') {
        Some(i) => &path[..i],
        None => path,
    };
    let mut segs = structural.split('/');
    if segs.next() != Some("root") {
        return None;
    }
    let mut out = Vec::new();
    for s in segs {
        out.push(s.parse::<usize>().ok()?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(json: &str) -> Document {
        Document::from_json(json).unwrap()
    }

    fn sample() -> Document {
        doc(r#"{
            "format": 1, "kind": "llama:pause", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "label", "text": "Paused" },
                { "type": "row", "children": [
                    { "type": "button", "id": "a", "text": "A" },
                    { "type": "button", "id": "a", "text": "B" }
                ] }
            ] }
        }"#)
    }

    #[test]
    fn issue_paths_resolve_to_the_offending_node() {
        let d = sample();
        // The duplicate-id issue is anchored at the second button.
        let issues = d.validate(None, None);
        let dup = issues.iter().find(|i| i.message.contains("duplicate id")).unwrap();
        let path = resolve_issue_path(&dup.path).expect("path parses");
        let node = node_at(&d.root, &path).expect("path resolves");
        assert_eq!(node.kind.type_name(), "button");
        assert_eq!(path, vec![1, 1]);
        // Document-level issues select the root.
        assert_eq!(resolve_issue_path("document"), Some(vec![]));
        assert_eq!(resolve_issue_path("root(frame)"), Some(vec![]));
        assert_eq!(resolve_issue_path("bogus/1"), None);
    }

    #[test]
    fn move_node_adjusts_shifted_indices() {
        let mut d = sample();
        // Move the label (0) to the end of the row (1 -> shifts to 0 after removal).
        let new = move_node(&mut d.root, &[0], &[1], 2).unwrap();
        assert_eq!(new, vec![0, 2]);
        assert_eq!(d.root.children.len(), 1);
        assert_eq!(d.root.children[0].children.len(), 3);
        assert_eq!(d.root.children[0].children[2].kind.type_name(), "label");
        // Refuses to move a node into its own subtree.
        assert!(move_node(&mut d.root, &[0], &[0, 0], 0).is_none());
    }

    #[test]
    fn unique_ids_never_collide() {
        let d = sample();
        assert_eq!(unique_id(&d, "b"), "b");
        assert_eq!(unique_id(&d, "a"), "a2");
        let mut copy = d.root.children[1].clone();
        uniquify_ids(&d, &mut copy);
        let ids: Vec<_> = [&copy.children[0], &copy.children[1]]
            .iter()
            .map(|n| n.id.clone().unwrap())
            .collect();
        assert!(!ids.contains(&"a".to_owned()), "{ids:?}");
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn wrap_in_keeps_the_node() {
        let mut d = sample();
        wrap_in(&mut d.root, &[0], NodeKind::Row).unwrap();
        assert_eq!(d.root.children[0].kind.type_name(), "row");
        assert_eq!(d.root.children[0].children[0].kind.type_name(), "label");
    }

    #[test]
    fn missing_image_issues_point_at_the_node_and_resolve() {
        let d = doc(r#"{
            "format": 1, "kind": "llama:pause", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "image", "image": "ok.png" },
                { "type": "row", "children": [
                    { "type": "image", "image": "missing.png" }
                ] }
            ] }
        }"#);
        let issues = missing_image_issues(&d, &|name| name == "ok.png");
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("missing.png"));
        // The panel's click-to-select resolver understands the path.
        let path = resolve_issue_path(&issues[0].path).unwrap();
        assert_eq!(path, vec![1, 0]);
        assert_eq!(node_at(&d.root, &path).unwrap().kind.type_name(), "image");
    }
}
