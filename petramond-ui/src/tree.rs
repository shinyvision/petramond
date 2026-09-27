//! Document → instance-tree expansion.
//!
//! A document is authored once; each frame it expands against the host's
//! [`UiState`] into a flat arena of instances: list templates are repeated per
//! item, `visible: false` nodes are dropped (they take no space), and every
//! binding is resolved to a concrete value. Layout, widgets, and paint all
//! run over this arena, so binding resolution happens in exactly one place.
//!
//! Every instance records which state keys its expansion read. That is what
//! lets the runtime keep last frame's arena and re-expand only the subtrees
//! whose inputs changed (see [`reuse`]): a clean subtree moves across frames
//! wholesale instead of being resolved again.

pub(crate) mod reuse;

use crate::doc::{Document, Node, NodeKind};
use crate::state::{UiMap, UiState, UiValue};
use reuse::Prev;

/// A stable per-frame identity for an id-bearing instance: the node id plus
/// the list item index when the node lives inside a template. Ephemeral
/// widget state (hover, focus, scroll, editors) keys off this.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstKey {
    pub id: String,
    pub item: Option<u32>,
}

/// One expanded node instance: the document node it stamps, plus every
/// binding resolved ([`InstData`], reachable directly through `Deref`).
#[derive(Debug)]
pub struct Inst<'d> {
    pub node: &'d Node,
    /// The layout this instance arranges by: the node's `compact_layout` when
    /// the tree expanded in compact form (and the node carries one), else its
    /// ordinary `layout`.
    pub layout: &'d crate::doc::LayoutProps,
    data: InstData,
}

impl std::ops::Deref for Inst<'_> {
    type Target = InstData;

    fn deref(&self) -> &InstData {
        &self.data
    }
}

/// An instance's resolved state, independent of the document borrow — the
/// part that survives from one frame to the next.
#[derive(Debug)]
pub struct InstData {
    /// Pre-order index of the stamped node in its document.
    pub node_id: u32,
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
    /// Resolved `frame` binding: the authoritative sprite-sheet frame index
    /// (already truncated to an integer; the paint walk clamps it into the
    /// sheet's grid).
    pub frame: Option<i32>,
    /// Resolved `tint` binding as a linear multiply colour.
    pub tint: Option<[f32; 4]>,
    /// Resolved `item` binding (hook nodes): the game item to draw in the
    /// hook's rect (empty string resolves to `None` — nothing to show).
    pub item_name: Option<String>,
    /// Resolved `min_w` binding: this frame's minimum width for a box whose
    /// content the layout engine cannot measure (host-drawn hooks).
    pub min_w: Option<i32>,
    /// Resolved `abs_x`/`abs_y` bindings: per-frame overrides of the node's
    /// authored `layout.abs` position.
    pub abs_x: Option<i32>,
    pub abs_y: Option<i32>,
    /// Resolved `palette` binding (labels, badges, inputs): the theme palette
    /// entry that colours the node this frame (empty resolves to `None`).
    pub palette: Option<String>,
    /// Resolved `scene` binding (canvas nodes): the host scene to paint.
    pub scene: Option<String>,
    /// Resolved `icon` binding: this frame's icon name (empty = `None`).
    pub icon: Option<String>,
    pub text_opacity: f32,
    pub enabled: bool,
    /// The enabled state inherited from the parent when this was expanded.
    pub(crate) parent_enabled: bool,
    /// Arena index of the parent instance (`None` for the root).
    pub parent: Option<u32>,
    /// Arena indices of this instance's children, in document/item order.
    pub children: Vec<u32>,
    /// Identity for ephemeral state + events (id-bearing nodes only).
    pub key: Option<InstKey>,
    /// Instances in this subtree, itself included (it is contiguous in the
    /// arena: pre-order).
    pub(crate) span: u32,
    /// Hashes of every state key this instance's expansion read — its own
    /// bindings plus the visibility of children that expanded to nothing.
    pub(crate) deps: Vec<u64>,
}

/// The expanded arena. Index 0 is the root.
#[derive(Debug)]
pub struct InstTree<'d> {
    pub insts: Vec<Inst<'d>>,
}

pub const ROOT: u32 = 0;

/// `0xRRGGBB` to a linear multiply colour. Packed as an `I32` because the GUI
/// state map carries no colour type and a publisher already has the bytes.
fn unpack_tint(packed: i32) -> [f32; 4] {
    let v = packed as u32;
    [
        ((v >> 16) & 0xFF) as f32 / 255.0,
        ((v >> 8) & 0xFF) as f32 / 255.0,
        (v & 0xFF) as f32 / 255.0,
        1.0,
    ]
}

impl Inst<'_> {
    /// The effective flow direction for this instance's children.
    pub fn flow_dir(&self) -> crate::doc::Dir {
        self.node.flow_dir_of(self.layout)
    }

    /// The effective cross-axis alignment of this instance's children.
    pub fn effective_align(&self) -> crate::doc::Align {
        self.node.effective_align_of(self.layout)
    }

    /// This frame's icon name for a `button`/`toggle`: the bound override,
    /// else the authored one.
    pub fn icon_name(&self) -> Option<&str> {
        if let Some(icon) = self.icon.as_deref() {
            return Some(icon);
        }
        match &self.node.kind {
            NodeKind::Button { icon, .. } | NodeKind::Toggle { icon } => icon.as_deref(),
            _ => None,
        }
    }

    /// The effective image name for `image`/`rotimage`/image-backed `button`
    /// nodes: the bound override, else the node's static name (`None` when
    /// empty).
    pub fn image_name(&self) -> Option<&str> {
        if let Some(name) = self.image.as_deref() {
            return (!name.is_empty()).then_some(name);
        }
        match &self.node.kind {
            NodeKind::Image { image, .. } | NodeKind::Rotimage { image, .. } => {
                (!image.is_empty()).then_some(image.as_str())
            }
            NodeKind::Button {
                image: Some(image), ..
            } => (!image.is_empty()).then_some(image.as_str()),
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
        Self::expand_form_hover(doc, state, compact, None)
    }

    /// [`expand_form`](Self::expand_form) with the widget under the cursor
    /// (one frame old — the same contract as hover-revealed list content): a
    /// tooltip whose `hover` anchor does not match expands as invisible, so an
    /// anchored tooltip costs nothing on the frames its widget is not pointed
    /// at. A tooltip stamped inside a list template matches only its own
    /// stamp's widget, and binds that stamp's values; one outside matches the
    /// widget id on any stamp.
    pub fn expand_form_hover(
        doc: &'d Document,
        state: &UiState,
        compact: bool,
        hover: Option<&InstKey>,
    ) -> InstTree<'d> {
        let shape = reuse::DocShape::of(doc);
        Self::expand_with(doc, &shape, state, compact, hover, None)
    }

    /// The expansion every entry point shares: `prev` (last frame's arena of
    /// the same document, with its dirty subtrees marked) donates every clean
    /// subtree instead of it being resolved again.
    pub(crate) fn expand_with(
        doc: &'d Document,
        shape: &reuse::DocShape<'d>,
        state: &UiState,
        compact: bool,
        hover: Option<&InstKey>,
        prev: Option<&mut Prev>,
    ) -> InstTree<'d> {
        let mut tree = InstTree { insts: Vec::new() };
        let mut grow = Grow {
            tree: &mut tree,
            shape,
            state,
            compact,
            hover,
            prev,
        };
        grow.node(
            &doc.root,
            At {
                node_id: 0,
                item_map: None,
                item: None,
                parent: None,
                parent_enabled: true,
                counterpart: Some(ROOT),
                reusable: true,
            },
        );
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

    /// Detach the arena from the document borrow, keeping every resolved
    /// value for the next frame.
    pub(crate) fn into_data(self) -> Vec<InstData> {
        self.insts.into_iter().map(|inst| inst.data).collect()
    }

    /// Re-attach a detached arena to its document (`shape` indexes the same
    /// document the data was expanded from).
    pub(crate) fn from_data(
        shape: &reuse::DocShape<'d>,
        data: Vec<InstData>,
        compact: bool,
    ) -> InstTree<'d> {
        let insts = data
            .into_iter()
            .map(|data| {
                let node = shape.node(data.node_id);
                Inst {
                    node,
                    layout: node.layout_for(compact),
                    data,
                }
            })
            .collect();
        InstTree { insts }
    }
}

/// Where one node is being stamped: the list item it resolves against, its
/// parent, and its counterpart in last frame's arena (if any).
#[derive(Clone, Copy)]
struct At<'m> {
    node_id: u32,
    item_map: Option<&'m UiMap>,
    item: Option<u32>,
    parent: Option<u32>,
    parent_enabled: bool,
    /// The previous arena's instance of this same node and item, when one
    /// exists and may be matched.
    counterpart: Option<u32>,
    /// Whether clean previous subtrees may be adopted here. Off below a list
    /// that re-expanded: its rows may resolve against new item maps even when
    /// no global key they read changed.
    reusable: bool,
}

/// One expansion pass over a document.
struct Grow<'t, 'd, 's> {
    tree: &'t mut InstTree<'d>,
    shape: &'s reuse::DocShape<'d>,
    state: &'s UiState,
    compact: bool,
    hover: Option<&'s InstKey>,
    prev: Option<&'s mut Prev>,
}

/// Binding reads for one instance, recording every key they touch.
struct Reads<'a> {
    state: &'a UiState,
    item: Option<&'a UiMap>,
    deps: Vec<u64>,
}

impl<'a> Reads<'a> {
    fn key(&mut self, key: &Option<String>) -> Option<&'a UiValue> {
        let key = key.as_deref()?;
        self.deps.push(reuse::key_hash(key));
        self.state.resolve(self.item, key)
    }

    fn bool(&mut self, key: &Option<String>, default: bool) -> bool {
        self.key(key).and_then(UiValue::as_bool).unwrap_or(default)
    }

    fn str(&mut self, key: &Option<String>) -> Option<String> {
        match self.key(key)? {
            UiValue::Str(s) if !s.is_empty() => Some(s.clone()),
            _ => None,
        }
    }

    fn int(&mut self, key: &Option<String>) -> Option<i32> {
        self.key(key).and_then(UiValue::as_f32).map(|f| f as i32)
    }

    fn text(&mut self, node: &Node) -> Option<String> {
        let bound = self.key(&node.bind.text).and_then(UiValue::as_display_text);
        if bound.is_some() {
            return bound;
        }
        match &node.kind {
            NodeKind::Label { text, .. }
            | NodeKind::Button { text, .. }
            | NodeKind::Badge { text }
            | NodeKind::Alert { text, .. } => text.clone(),
            // Inputs show their bound text (editor overlays it while focused).
            _ => None,
        }
    }
}

impl<'d> Grow<'_, 'd, '_> {
    /// Expand `node` (and descendants) into the arena; returns its index, or
    /// `None` when the node resolved invisible.
    fn node(&mut self, node: &'d Node, at: At<'_>) -> Option<u32> {
        if let Some(adopted) = self.adopt(at) {
            return Some(adopted);
        }
        let mut reads = Reads {
            state: self.state,
            item: at.item_map,
            deps: Vec::new(),
        };
        let visible = reads.bool(&node.bind.visible, true);
        let anchored = match &node.kind {
            NodeKind::Tooltip {
                hover: Some(anchor),
            } => Some(anchor.as_str()),
            _ => None,
        };
        if anchored.is_some() {
            reads.deps.push(reuse::HOVER_DEP);
        }
        // A tooltip stamped inside a list template matches only its own
        // stamp's widget; one outside matches the widget id on any stamp.
        let hover = self.hover;
        let hovered = |anchor: &str| {
            hover.is_some_and(|h| h.id == anchor && (at.item.is_none() || h.item == at.item))
        };
        if !visible || anchored.is_some_and(|anchor| !hovered(anchor)) {
            // The parent must know what hid this child, or a clean parent
            // could be adopted next frame without the child that should
            // have appeared.
            if let Some(p) = at.parent {
                self.tree.insts[p as usize]
                    .data
                    .deps
                    .append(&mut reads.deps);
            }
            return None;
        }
        let idx = self.tree.insts.len() as u32;
        let items = match (&node.kind, reads.key(&node.bind.items)) {
            (NodeKind::List { .. }, Some(UiValue::List(items))) => Some(items.clone()),
            _ => None,
        };
        let value = reads.key(&node.bind.value);
        let data = InstData {
            node_id: at.node_id,
            item: at.item,
            text: reads.text(node),
            value_f32: value.and_then(UiValue::as_f32),
            value_bool: value.and_then(UiValue::as_bool),
            selected: match reads.key(&node.bind.selected) {
                Some(UiValue::I32(i)) => Some(*i),
                _ => None,
            },
            enabled: at.parent_enabled && reads.bool(&node.bind.enabled, true),
            parent_enabled: at.parent_enabled,
            image: match reads.key(&node.bind.image) {
                Some(UiValue::Str(s)) => Some(s.clone()),
                _ => None,
            },
            frame: reads.int(&node.bind.frame),
            tint: match reads.key(&node.bind.tint) {
                Some(UiValue::I32(packed)) => Some(unpack_tint(*packed)),
                _ => None,
            },
            item_name: reads.str(&node.bind.item),
            min_w: reads.int(&node.bind.min_w),
            abs_x: reads.int(&node.bind.abs_x),
            abs_y: reads.int(&node.bind.abs_y),
            palette: reads.str(&node.bind.palette),
            scene: reads.str(&node.bind.scene),
            icon: reads.str(&node.bind.icon),
            text_opacity: reads
                .key(&node.bind.text_opacity)
                .and_then(UiValue::as_f32)
                .filter(|v| v.is_finite())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0),
            parent: at.parent,
            children: Vec::new(),
            key: node.id.as_ref().map(|id| InstKey {
                id: id.clone(),
                item: at.item,
            }),
            span: 1,
            deps: std::mem::take(&mut reads.deps),
        };
        let enabled = data.enabled;
        self.tree.insts.push(Inst {
            node,
            layout: node.layout_for(self.compact),
            data,
        });

        let mut children = Vec::new();
        match &node.kind {
            NodeKind::List { .. } => {
                if let (Some(template), Some(items)) = (node.children.first(), items) {
                    let template_id = at.node_id + 1;
                    for (i, m) in items.iter().enumerate() {
                        let item = Some(i as u32);
                        let child = At {
                            node_id: template_id,
                            item_map: Some(m),
                            item,
                            parent: Some(idx),
                            parent_enabled: enabled,
                            counterpart: None,
                            reusable: false,
                        };
                        children.extend(self.node(template, child));
                    }
                }
            }
            _ => {
                let mut child_id = at.node_id + 1;
                for child in &node.children {
                    let child_at = At {
                        node_id: child_id,
                        item_map: at.item_map,
                        item: at.item,
                        parent: Some(idx),
                        parent_enabled: enabled,
                        counterpart: self.counterpart_of(at.counterpart, child_id, at.item),
                        reusable: at.reusable,
                    };
                    children.extend(self.node(child, child_at));
                    child_id += self.shape.size(child_id);
                }
            }
        }
        let span = self.tree.insts.len() as u32 - idx;
        let inst = &mut self.tree.insts[idx as usize].data;
        inst.children = children;
        inst.span = span;
        Some(idx)
    }

    /// The previous arena's child of `parent` stamping node `node_id` for
    /// `item`, if the previous frame had one.
    fn counterpart_of(&self, parent: Option<u32>, node_id: u32, item: Option<u32>) -> Option<u32> {
        let prev = self.prev.as_deref()?;
        prev.child_of(parent?, node_id, item)
    }

    /// Move the previous frame's instance of this node (and its whole
    /// subtree) into the arena when nothing it read has changed.
    fn adopt(&mut self, at: At<'_>) -> Option<u32> {
        if !at.reusable {
            return None;
        }
        let prev = self.prev.as_deref_mut()?;
        let j = at.counterpart?;
        if !prev.is_clean(j, at.node_id, at.item, at.parent_enabled) {
            return None;
        }
        let new_start = self.tree.insts.len() as u32;
        for data in prev.take_subtree(j, new_start, at.parent) {
            let node = self.shape.node(data.node_id);
            self.tree.insts.push(Inst {
                node,
                layout: node.layout_for(self.compact),
                data,
            });
        }
        Some(new_start)
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
