use super::InstData;
use crate::doc::{Document, Node};
use std::collections::HashSet;

pub(crate) const HOVER_DEP: u64 = 0x686f_7665_725f_6465;

pub(crate) fn key_hash(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

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

    pub(crate) fn size(&self, id: u32) -> u32 {
        self.sizes[id as usize]
    }
}

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

pub(crate) struct Prev {
    insts: Vec<InstData>,
    dirty: Vec<bool>,
    adopted: usize,
}

impl Prev {
    pub(crate) fn new(insts: Vec<InstData>, changed: &HashSet<u64>) -> Prev {
        let mut dirty: Vec<bool> = insts
            .iter()
            .map(|d| d.deps.iter().any(|h| changed.contains(h)))
            .collect();
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

    pub(crate) fn adopted(&self) -> usize {
        self.adopted
    }

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
