//! Document → instance-tree expansion.
//!
//! A document is authored once; each frame it expands against the host's
//! [`UiState`] into a flat arena of instances: list templates are repeated per
//! item, `visible: false` nodes are dropped (they take no space), and every
//! binding is resolved to a concrete value. Layout, widgets, and paint all
//! run over this arena, so binding resolution happens in exactly one place.

use crate::doc::{Document, Node, NodeKind};
use crate::state::{UiMap, UiState, UiValue};

/// A stable per-frame identity for an id-bearing instance: the node id plus
/// the list item index when the node lives inside a template. Ephemeral
/// widget state (hover, focus, scroll, editors) keys off this.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstKey {
    pub id: String,
    pub item: Option<u32>,
}

/// One expanded node instance with every binding resolved.
#[derive(Debug)]
pub struct Inst<'d> {
    pub node: &'d Node,
    /// The layout this instance arranges by: the node's `compact_layout` when
    /// the tree expanded in compact form (and the node carries one), else its
    /// ordinary `layout`.
    pub layout: &'d crate::doc::LayoutProps,
    /// The innermost list item index this instance was stamped from.
    pub item: Option<u32>,
    /// Resolved display text (label/button/badge/alert): binding, else static.
    pub text: Option<String>,
    /// Resolved `value` binding as f32 (gauge fraction, slider value,
    /// rotimage radians).
    pub value_f32: Option<f32>,
    /// Resolved `value` binding as bool (checkbox/toggle on-state).
    pub value_bool: Option<bool>,
    /// Resolved `selected` binding (list selection index; −1 = none).
    pub selected: Option<i32>,
    /// Resolved `image` binding: per-instance image-name override.
    pub image: Option<String>,
    pub enabled: bool,
    /// Arena index of the parent instance (`None` for the root).
    pub parent: Option<u32>,
    /// Arena indices of this instance's children, in document/item order.
    pub children: Vec<u32>,
    /// Identity for ephemeral state + events (id-bearing nodes only).
    pub key: Option<InstKey>,
}

/// The expanded arena. Index 0 is the root.
#[derive(Debug)]
pub struct InstTree<'d> {
    pub insts: Vec<Inst<'d>>,
}

pub const ROOT: u32 = 0;

impl Inst<'_> {
    /// The effective flow direction for this instance's children.
    pub fn flow_dir(&self) -> crate::doc::Dir {
        self.node.flow_dir_of(self.layout)
    }

    /// The effective cross-axis alignment of this instance's children.
    pub fn effective_align(&self) -> crate::doc::Align {
        self.node.effective_align_of(self.layout)
    }

    /// The effective image name for `image`/`rotimage` nodes: the bound
    /// override, else the node's static name (`None` when empty).
    pub fn image_name(&self) -> Option<&str> {
        if let Some(name) = self.image.as_deref() {
            return (!name.is_empty()).then_some(name);
        }
        match &self.node.kind {
            NodeKind::Image { image, .. } | NodeKind::Rotimage { image, .. } => {
                (!image.is_empty()).then_some(image.as_str())
            }
            _ => None,
        }
    }
}

impl<'d> InstTree<'d> {
    pub fn expand(doc: &'d Document, state: &UiState) -> InstTree<'d> {
        Self::expand_form(doc, state, false)
    }

    /// Expand in normal or compact form (the caller resolves the document's
    /// breakpoint against its viewport — see [`Document::compact_active`]).
    pub fn expand_form(doc: &'d Document, state: &UiState, compact: bool) -> InstTree<'d> {
        let mut tree = InstTree { insts: Vec::new() };
        tree.grow(&doc.root, state, None, None, None, true, compact);
        tree
    }

    pub fn root(&self) -> &Inst<'d> {
        &self.insts[ROOT as usize]
    }

    pub fn get(&self, i: u32) -> &Inst<'d> {
        &self.insts[i as usize]
    }

    pub fn len(&self) -> usize {
        self.insts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.insts.is_empty()
    }

    /// The arena index of the instance keyed `id` (+ optional item), if it
    /// expanded this frame.
    pub fn find(&self, id: &str, item: Option<u32>) -> Option<u32> {
        self.insts
            .iter()
            .position(|inst| {
                inst.key
                    .as_ref()
                    .is_some_and(|k| k.id == id && k.item == item)
            })
            .map(|i| i as u32)
    }

    /// Expand `node` (and descendants) into the arena; returns its index, or
    /// `None` when the node resolved invisible.
    #[allow(clippy::too_many_arguments)]
    fn grow(
        &mut self,
        node: &'d Node,
        state: &UiState,
        item_map: Option<&UiMap>,
        item: Option<u32>,
        parent: Option<u32>,
        parent_enabled: bool,
        compact: bool,
    ) -> Option<u32> {
        if !resolve_bool(state, item_map, &node.bind.visible, true) {
            return None;
        }
        let idx = self.insts.len() as u32;
        self.insts.push(Inst {
            node,
            layout: node.layout_for(compact),
            item,
            text: resolve_text(state, item_map, node),
            value_f32: resolve_key(state, item_map, &node.bind.value).and_then(UiValue::as_f32),
            value_bool: resolve_key(state, item_map, &node.bind.value).and_then(UiValue::as_bool),
            selected: match resolve_key(state, item_map, &node.bind.selected) {
                Some(UiValue::I32(i)) => Some(*i),
                _ => None,
            },
            enabled: parent_enabled && resolve_bool(state, item_map, &node.bind.enabled, true),
            image: resolve_key(state, item_map, &node.bind.image).and_then(|v| match v {
                UiValue::Str(s) => Some(s.clone()),
                _ => None,
            }),
            parent,
            children: Vec::new(),
            key: node.id.as_ref().map(|id| InstKey {
                id: id.clone(),
                item,
            }),
        });

        let enabled = self.insts[idx as usize].enabled;
        let child_indices = match &node.kind {
            NodeKind::List => {
                let template = node.children.first();
                let items =
                    node.bind
                        .items
                        .as_deref()
                        .and_then(|k| match state.resolve(item_map, k) {
                            Some(UiValue::List(items)) => Some(items.clone()),
                            _ => None,
                        });
                let mut out = Vec::new();
                if let (Some(template), Some(items)) = (template, items) {
                    for (i, m) in items.iter().enumerate() {
                        if let Some(ci) = self.grow(
                            template,
                            state,
                            Some(m),
                            Some(i as u32),
                            Some(idx),
                            enabled,
                            compact,
                        ) {
                            out.push(ci);
                        }
                    }
                }
                out
            }
            _ => node
                .children
                .iter()
                .filter_map(|c| self.grow(c, state, item_map, item, Some(idx), enabled, compact))
                .collect(),
        };
        self.insts[idx as usize].children = child_indices;
        Some(idx)
    }
}

fn resolve_key<'a>(
    state: &'a UiState,
    item: Option<&'a UiMap>,
    key: &Option<String>,
) -> Option<&'a UiValue> {
    key.as_deref().and_then(|k| state.resolve(item, k))
}

fn resolve_bool(
    state: &UiState,
    item: Option<&UiMap>,
    key: &Option<String>,
    default: bool,
) -> bool {
    match resolve_key(state, item, key) {
        Some(v) => v.as_bool().unwrap_or(default),
        None => default,
    }
}

fn resolve_text(state: &UiState, item: Option<&UiMap>, node: &Node) -> Option<String> {
    let bound = resolve_key(state, item, &node.bind.text).and_then(UiValue::as_display_text);
    if bound.is_some() {
        return bound;
    }
    match &node.kind {
        NodeKind::Label { text, .. }
        | NodeKind::Button { text, .. }
        | NodeKind::Badge { text }
        | NodeKind::Alert { text, .. } => text.clone(),
        NodeKind::TextInput { .. } => {
            // Inputs show their bound text (editor overlays it while focused).
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Document;
    use crate::state::{UiMap, UiState, UiValue};
    use std::sync::Arc;

    fn list_doc() -> Document {
        Document::from_json(
            r#"{
            "format": 1, "kind": "petramond:world_settings", "class": "screen",
            "root": { "type": "column", "children": [
                { "type": "label", "text": "Mods", "bind": { "text": "heading" } },
                { "type": "list", "id": "mods", "bind": { "items": "mod_rows", "selected": "mod_sel" },
                  "children": [
                    { "type": "row", "children": [
                        { "type": "label", "bind": { "text": "name" } },
                        { "type": "toggle", "id": "mod_on", "bind": { "value": "enabled", "enabled": "toggleable" } }
                    ] }
                ] },
                { "type": "button", "id": "back", "text": "Back", "bind": { "visible": "show_back" } }
            ] }
        }"#,
        )
        .unwrap()
    }

    fn row(name: &str, enabled: bool, toggleable: bool) -> UiMap {
        let mut m = UiMap::new();
        m.insert("name".into(), UiValue::Str(name.into()));
        m.insert("enabled".into(), UiValue::Bool(enabled));
        m.insert("toggleable".into(), UiValue::Bool(toggleable));
        m
    }

    #[test]
    fn lists_stamp_the_template_per_item_with_item_bindings() {
        let doc = list_doc();
        let mut state = UiState::new();
        state.set(
            "mod_rows",
            UiValue::List(Arc::new(vec![
                row("Weather Pack", true, true),
                row("Zombies", false, false),
            ])),
        );
        state.set("mod_sel", UiValue::I32(1));
        let tree = InstTree::expand(&doc, &state);

        let list_idx = tree.find("mods", None).expect("list expands");
        let list = tree.get(list_idx);
        assert_eq!(list.children.len(), 2, "one template stamp per item");
        assert_eq!(list.selected, Some(1));

        // First row: label text from item map, toggle on + enabled.
        let row0 = tree.get(list.children[0]);
        let label0 = tree.get(row0.children[0]);
        assert_eq!(label0.text.as_deref(), Some("Weather Pack"));
        let toggle0 = tree.get(row0.children[1]);
        assert_eq!(toggle0.value_bool, Some(true));
        assert!(toggle0.enabled);
        assert_eq!(
            toggle0.key,
            Some(InstKey {
                id: "mod_on".into(),
                item: Some(0)
            })
        );

        // Second row: distinct key, off + disabled.
        let row1 = tree.get(list.children[1]);
        let toggle1 = tree.get(row1.children[1]);
        assert_eq!(toggle1.value_bool, Some(false));
        assert!(!toggle1.enabled);
        assert_eq!(toggle1.key.as_ref().unwrap().item, Some(1));

        // Instance lookup by (id, item) resolves to the per-item stamp.
        assert_eq!(tree.find("mod_on", Some(1)), Some(row1.children[1]));
        assert_eq!(tree.find("mod_on", Some(7)), None);
    }

    #[test]
    fn bound_text_overrides_static_and_missing_items_key_means_empty_list() {
        let doc = list_doc();
        let mut state = UiState::new();
        state.set("heading", UiValue::Str("Installed Mods".into()));
        let tree = InstTree::expand(&doc, &state);
        let root = tree.root();
        let heading = tree.get(root.children[0]);
        assert_eq!(heading.text.as_deref(), Some("Installed Mods"));
        let list = tree.get(tree.find("mods", None).unwrap());
        assert_eq!(list.children.len(), 0, "no items bound -> zero stamps");
    }

    #[test]
    fn invisible_nodes_are_dropped_entirely() {
        let doc = list_doc();
        let mut state = UiState::new();
        state.set("show_back", UiValue::Bool(false));
        let tree = InstTree::expand(&doc, &state);
        assert_eq!(tree.find("back", None), None);
        // Default (key absent) is visible.
        let tree = InstTree::expand(&doc, &UiState::new());
        assert!(tree.find("back", None).is_some());
    }

    #[test]
    fn disabled_ancestor_disables_every_descendant() {
        let doc = Document::from_json(
            r#"{
            "format": 1, "kind": "petramond:x", "class": "screen",
            "root": { "type": "frame", "bind": { "enabled": "panel_on" },
                "children": [
                    { "type": "button", "id": "action", "text": "Action",
                      "bind": { "enabled": "action_on" } }
                ] }
        }"#,
        )
        .unwrap();
        let mut state = UiState::new();
        state.set("panel_on", UiValue::Bool(false));
        state.set("action_on", UiValue::Bool(true));
        let tree = InstTree::expand(&doc, &state);
        assert!(!tree.get(0).enabled);
        assert!(!tree.get(1).enabled);

        state.set("panel_on", UiValue::Bool(true));
        let tree = InstTree::expand(&doc, &state);
        assert!(tree.get(0).enabled);
        assert!(tree.get(1).enabled);
    }
}
