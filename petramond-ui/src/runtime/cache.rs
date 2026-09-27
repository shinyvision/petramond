//! The per-GUI frame cache: last frame's expanded arena and solved layout,
//! kept in the host's [`FrameState`](crate::FrameState) between frames.
//!
//! Expansion is keyed on the document, the state's `(origin, revision)`,
//! the compact form and the hover anchor. An exact match reuses the whole
//! arena; a later revision of the same state re-expands only the subtrees
//! that read a changed key (see [`crate::tree::reuse`]). Layout is keyed on
//! the expansion plus everything else the solver reads — theme, scale,
//! viewport, scroll offsets and image sizes — and is skipped entirely when
//! nothing moved, leaving only interaction and paint to run.

use crate::doc::{Document, NodeKind};
use crate::input::FrameState;
use crate::layout::Solved;
use crate::paint_walk::DocImages;
use crate::state::UiState;
use crate::theme::Theme;
use crate::tree::reuse::{changed_deps, DocShape, Prev};
use crate::tree::{InstData, InstTree};
use std::sync::Arc;

/// What the last frame's cache did — for profiling and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Instances resolved from bindings this frame.
    pub expanded: usize,
    /// Instances carried over unchanged from the previous frame.
    pub reused: usize,
    /// Whether the solved layout was reused instead of solved again.
    pub layout_reused: bool,
}

/// The inputs an expansion is a pure function of (besides the document).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExpandKey {
    pub origin: u64,
    pub revision: u64,
    pub compact: bool,
    pub hover: Option<crate::tree::InstKey>,
}

/// The inputs a solve is a pure function of (besides document and theme).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayoutKey {
    pub expand: ExpandKey,
    pub scale: i32,
    pub viewport: (i32, i32),
    /// Hash of every scroll instance's offset.
    pub scrolls: u64,
    /// Hash of every image instance's resolved natural size.
    pub images: u64,
}

#[derive(Default)]
pub(crate) struct FrameCache {
    doc: Option<Arc<Document>>,
    theme: Option<Arc<Theme>>,
    expanded: Option<ExpandKey>,
    insts: Vec<InstData>,
    layout: Option<(LayoutKey, Solved)>,
    pub stats: CacheStats,
}

impl std::fmt::Debug for FrameCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCache")
            .field("instances", &self.insts.len())
            .field("layout", &self.layout.is_some())
            .field("stats", &self.stats)
            .finish()
    }
}

impl FrameCache {
    /// Point the cache at this frame's document and theme, dropping whatever
    /// belonged to different ones (a node id means nothing across documents;
    /// a layout means nothing across themes).
    pub fn bind(&mut self, doc: &Arc<Document>, theme: &Arc<Theme>) {
        if !self.doc.as_ref().is_some_and(|d| Arc::ptr_eq(d, doc)) {
            self.doc = Some(doc.clone());
            self.expanded = None;
            self.insts.clear();
            self.layout = None;
        }
        if !self.theme.as_ref().is_some_and(|t| Arc::ptr_eq(t, theme)) {
            self.theme = Some(theme.clone());
            self.layout = None;
        }
        self.stats = CacheStats::default();
    }

    /// This frame's arena: the cached one when `key` matches exactly, else
    /// an expansion that adopts every clean subtree of the cached one.
    pub fn expand<'d>(
        &mut self,
        doc: &'d Document,
        shape: &DocShape<'d>,
        state: &UiState,
        key: ExpandKey,
    ) -> InstTree<'d> {
        let prev = std::mem::take(&mut self.insts);
        let tree = match self.expanded.take() {
            Some(old) if old == key => {
                self.stats.reused = prev.len();
                InstTree::from_data(shape, prev, key.compact)
            }
            Some(old)
                if old.origin == key.origin
                    && old.compact == key.compact
                    && old.revision <= key.revision =>
            {
                let changed = changed_deps(state, old.revision, old.hover != key.hover);
                let mut prev = Prev::new(prev, &changed);
                let tree = InstTree::expand_with(
                    doc,
                    shape,
                    state,
                    key.compact,
                    key.hover.as_ref(),
                    Some(&mut prev),
                );
                self.stats.reused = prev.adopted();
                tree
            }
            _ => InstTree::expand_with(doc, shape, state, key.compact, key.hover.as_ref(), None),
        };
        self.stats.expanded = tree.len() - self.stats.reused;
        self.expanded = Some(key);
        tree
    }

    /// The cached layout, if it was solved from exactly `key`.
    pub fn take_layout(&mut self, key: &LayoutKey) -> Option<Solved> {
        match self.layout.take() {
            Some((cached, solved)) if cached == *key => {
                self.stats.layout_reused = true;
                Some(solved)
            }
            _ => None,
        }
    }

    /// Keep this frame's arena and (pristine, pre-tooltip-placement) layout
    /// for the next frame.
    pub fn store(&mut self, tree: InstTree<'_>, layout: Option<(LayoutKey, Solved)>) {
        self.insts = tree.into_data();
        self.layout = layout;
    }
}

/// Hashes of the non-tree layout inputs: every scroll instance's offset and
/// every image instance's resolved natural size.
pub(crate) fn layout_inputs(
    tree: &InstTree<'_>,
    fs: &FrameState,
    images: &dyn DocImages,
) -> (u64, u64) {
    use std::hash::{Hash, Hasher};
    let mut scrolls = std::collections::hash_map::DefaultHasher::new();
    let mut sizes = std::collections::hash_map::DefaultHasher::new();
    for (i, inst) in tree.insts.iter().enumerate() {
        if let (NodeKind::Scroll { .. }, Some(key)) = (&inst.node.kind, &inst.key) {
            (i, fs.scroll_offset(key)).hash(&mut scrolls);
        }
        if let Some(name) = inst.image_name() {
            (i, images.resolve(name).map(|(_, size)| size)).hash(&mut sizes);
        }
    }
    (scrolls.finish(), sizes.finish())
}

#[cfg(test)]
mod tests;
