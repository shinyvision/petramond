//! Frame-to-frame reuse of the expanded arena.
//!
//! Expansion is a pure function of the document, the state, the compact form
//! and the hover anchor. Every instance records the (hashed) state keys it
//! read, so given the keys that changed since the previous frame, a subtree
//! none of whose instances read a changed key expands exactly as it did — it
//! is MOVED from the previous arena instead of resolved again. A hash
//! collision can only make a subtree look dirty, never clean.

use super::InstData;
use crate::doc::{Document, Node};
use std::collections::HashSet;

/// The pseudo-key an anchored tooltip (and a parent that hid one) depends
/// on: the hover anchor, which lives outside the state map.
pub(crate) const HOVER_DEP: u64 = 0x686f_7665_725f_6465;

/// The dependency hash of a state key.
pub(crate) fn key_hash(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

/// A document's nodes in pre-order (their ids) with each subtree's size.
pub(crate) struct DocShape<'d> {
    nodes: Vec<&'d Node>,
    sizes: Vec<u32>,
}

impl<'d> DocShape<'d> {
    pub(crate) fn of(doc: &'d Document) -> DocShape<'d> {
        fn walk<'d>(node: &'d Node, shape: &mut DocShape<'d>) {
            let id = shape.nodes.len();
            shape.nodes.push(node);
            shape.sizes.push(1);
            for child in &node.children {
                walk(child, shape);
            }
            shape.sizes[id] = (shape.nodes.len() - id) as u32;
        }
        let mut shape = DocShape {
            nodes: Vec::new(),
            sizes: Vec::new(),
        };
        walk(&doc.root, &mut shape);
        shape
    }

    pub(crate) fn node(&self, id: u32) -> &'d Node {
        self.nodes[id as usize]
    }

    /// Nodes in the subtree rooted at `id`, itself included.
    pub(crate) fn size(&self, id: u32) -> u32 {
        self.sizes[id as usize]
    }
}

/// The dependency hashes that changed between two expansions: every state
/// key changed since `since`, plus [`HOVER_DEP`] when the hover anchor moved.
pub(crate) fn changed_deps(
    state: &crate::state::UiState,
    since: u64,
    hover_moved: bool,
) -> HashSet<u64> {
    let mut changed: HashSet<u64> = state.changed_since(since).map(key_hash).collect();
    if hover_moved {
        changed.insert(HOVER_DEP);
    }
    changed
}

/// The previous frame's arena, donating its clean subtrees.
pub(crate) struct Prev {
    insts: Vec<InstData>,
    /// Per previous instance: whether anything in its subtree read a key
    /// that changed.
    dirty: Vec<bool>,
    adopted: usize,
}

impl Prev {
    /// `changed` holds the hashes of every key that changed since `insts`
    /// was expanded (see [`changed_deps`]).
    pub(crate) fn new(insts: Vec<InstData>, changed: &HashSet<u64>) -> Prev {
        let mut dirty: Vec<bool> = insts
            .iter()
            .map(|d| d.deps.iter().any(|h| changed.contains(h)))
            .collect();
        // Children follow their parent in the arena, so one reverse pass
        // carries dirtiness up to every ancestor.
        for (i, inst) in insts.iter().enumerate().rev() {
            if let (true, Some(p)) = (dirty[i], inst.parent) {
                dirty[p as usize] = true;
            }
        }
        Prev {
            insts,
            dirty,
            adopted: 0,
        }
    }

    /// How many instances moved over instead of re-expanding.
    pub(crate) fn adopted(&self) -> usize {
        self.adopted
    }

    /// The child of `parent` stamping `node_id` for `item`, if any.
    pub(crate) fn child_of(&self, parent: u32, node_id: u32, item: Option<u32>) -> Option<u32> {
        self.insts
            .get(parent as usize)?
            .children
            .iter()
            .copied()
            .find(|&c| {
                let d = &self.insts[c as usize];
                d.node_id == node_id && d.item == item
            })
    }

    /// Whether instance `j` stamps the same node in the same context and
    /// nothing in its subtree read a changed key.
    pub(crate) fn is_clean(
        &self,
        j: u32,
        node_id: u32,
        item: Option<u32>,
        parent_enabled: bool,
    ) -> bool {
        self.insts.get(j as usize).is_some_and(|d| {
            d.node_id == node_id
                && d.item == item
                && d.parent_enabled == parent_enabled
                && !self.dirty[j as usize]
        })
    }

    /// Move instance `j`'s subtree out, re-indexed to start at `new_start`
    /// under `new_parent`.
    pub(crate) fn take_subtree(
        &mut self,
        j: u32,
        new_start: u32,
        new_parent: Option<u32>,
    ) -> impl Iterator<Item = InstData> + '_ {
        let span = self.insts[j as usize].span;
        self.adopted += span as usize;
        let shift = move |i: u32| (i as i64 - j as i64 + new_start as i64) as u32;
        (j..j + span).map(move |k| {
            let mut data = std::mem::replace(&mut self.insts[k as usize], vacant());
            data.parent = if k == j {
                new_parent
            } else {
                data.parent.map(shift)
            };
            for child in &mut data.children {
                *child = shift(*child);
            }
            data
        })
    }
}

/// What a moved-out slot holds: matches no node, so it can never be adopted
/// twice.
fn vacant() -> InstData {
    InstData {
        node_id: u32::MAX,
        item: None,
        text: None,
        value_f32: None,
        value_bool: None,
        selected: None,
        image: None,
        frame: None,
        tint: None,
        item_name: None,
        min_w: None,
        abs_x: None,
        abs_y: None,
        palette: None,
        scene: None,
        icon: None,
        text_opacity: 1.0,
        enabled: false,
        parent_enabled: false,
        parent: None,
        children: Vec::new(),
        key: None,
        span: 1,
        deps: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
