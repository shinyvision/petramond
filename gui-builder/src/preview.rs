//! The live preview pipeline: run the real petramond-ui runtime over the edited
//! document and rasterize its DrawList with the software rasterizer — the
//! preview *is* the game's renderer, pixel for pixel.
//!
//! The editor repaints on every pointer motion, so everything a repaint
//! reads is cached in [`PreviewCache`] by the document and theme revisions:
//! the decoded sample state, one runtime with its frame cache, and the
//! canvas hit-test rects. A repaint that changed nothing only draws.

use crate::assets::AssetRoots;
use crate::doc_edit::NodePath;
use crate::project::Project;
use petramond_ui::raster::TextureSet;
use petramond_ui::{
    DocImages, Document, FrameArgs, FrameOutput, FrameState, ImageData, InstTree, PreviewState,
    RectI, Theme, ThemeEnv, UiRuntime, UiState,
};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

// ---- document images ----------------------------------------------------------

/// What a [`DiskImages`] set was loaded for.
#[derive(Clone, Debug, Default, PartialEq)]
struct ImagesKey {
    names: Vec<String>,
    dir: Option<PathBuf>,
    roots: AssetRoots,
}

/// PNGs referenced by `image`/`rotimage` nodes and image-backed `button`
/// faces, loaded from beside the project, else from the asset layers (the
/// pack the document ships in, then the base game — where generated samples
/// find the shipped images they reference). Missing images resolve to
/// nothing and simply don't draw.
pub struct DiskImages {
    names: Vec<String>,
    images: Vec<ImageData>,
    key: ImagesKey,
}

impl DiskImages {
    pub fn empty() -> DiskImages {
        DiskImages {
            names: Vec::new(),
            images: Vec::new(),
            key: ImagesKey::default(),
        }
    }

    fn key(doc: &Document, dir: Option<&Path>, roots: &AssetRoots) -> ImagesKey {
        ImagesKey {
            names: petramond_ui::contract::image_refs(doc)
                .into_iter()
                .map(|r| r.name)
                .collect(),
            dir: dir.map(PathBuf::from),
            roots: roots.clone(),
        }
    }

    /// Reload if the document's image set, the project dir or the asset
    /// roots changed.
    pub fn refresh(&mut self, doc: &Document, dir: Option<&Path>, roots: &AssetRoots) {
        let key = Self::key(doc, dir, roots);
        if key == self.key {
            return;
        }
        *self = Self::load_key(key);
    }

    pub fn load(doc: &Document, dir: Option<&Path>, roots: &AssetRoots) -> DiskImages {
        Self::load_key(Self::key(doc, dir, roots))
    }

    fn load_key(key: ImagesKey) -> DiskImages {
        let mut names = Vec::new();
        let mut images = Vec::new();
        for name in &key.names {
            let Some(path) =
                crate::io::resolve_document_image_path(&key.roots, key.dir.as_deref(), name)
            else {
                continue;
            };
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let Ok(img) = image::load_from_memory(&bytes) else {
                continue;
            };
            let img = img.to_rgba8();
            let size = img.dimensions();
            names.push(name.clone());
            images.push(ImageData {
                rgba: img.into_raw(),
                size,
            });
        }
        DiskImages { names, images, key }
    }

    pub fn texture_refs(&self) -> Vec<&ImageData> {
        self.images.iter().collect()
    }
}

impl DocImages for DiskImages {
    fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))> {
        let i = self.names.iter().position(|n| n == name)?;
        Some((i as u16, self.images[i].size))
    }
}

// ---- rendering ------------------------------------------------------------------

/// Background behind the GUI in the preview (stands in for the 3D world).
pub const CLEAR: [u8; 4] = [30, 34, 40, 255];

/// Render one full preview frame of `rt`'s document to RGBA at `screen`
/// physical px. `fs` carries the runtime's frame cache between renders.
pub fn render_rgba(
    rt: &UiRuntime,
    fs: &mut FrameState,
    state: &UiState,
    images: &DiskImages,
    screen: (u32, u32),
    scale: i32,
    forced: Option<&PreviewState>,
) -> Vec<u8> {
    let mut out = FrameOutput::default();
    rt.frame(
        FrameArgs {
            screen,
            scale,
            now: 0.0,
            state,
            input: &[],
            clipboard: None,
            images,
            dim: None,
            preview: forced,
        },
        fs,
        &mut out,
    );
    let theme = rt.theme();
    let tex = TextureSet {
        theme_atlas: &theme.atlas,
        font: &theme.font,
        doc_images: &images.texture_refs(),
    };
    let mut rgba = Vec::new();
    petramond_ui::raster::rasterize(&out.draw, &tex, screen, CLEAR, &mut rgba);
    rgba
}

/// Render a project's preview (its own screen/scale settings), for the
/// `--screenshot` CLI and tests. `catalog` seeds sample data for unset keys,
/// exactly like the editor preview.
pub fn render_project(
    project: &Project,
    theme: &Arc<Theme>,
    roots: &AssetRoots,
    doc_dir: Option<&Path>,
    catalog: Option<&crate::bindings::Catalog>,
) -> (Vec<u8>, (u32, u32)) {
    let state = project.preview_state(catalog);
    let images = DiskImages::load(&project.document, doc_dir, roots);
    let screen = project.editor.screen;
    let scale = project.editor.preview_scale.clamp(1, 4) as i32;
    let rt = UiRuntime::new(Arc::new(project.document.clone()), theme.clone());
    let rgba = render_rgba(
        &rt,
        &mut FrameState::new(),
        &state,
        &images,
        screen,
        scale,
        None,
    );
    (rgba, screen)
}

// ---- the editor's cache -------------------------------------------------------------

/// What the canvas rects were solved for.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RectsKey {
    built: (u64, u64),
    viewport: (i32, i32),
    scale: i32,
}

/// The editor preview's work, kept across egui repaints and rebuilt only
/// when the document or theme revision moves: the preview state, one
/// runtime whose frame cache carries over between renders, and the canvas
/// hit-test rects.
#[derive(Default)]
pub struct PreviewCache {
    /// `(doc_rev, theme_rev)` the state and runtime were built for.
    built: Option<(u64, u64)>,
    state: UiState,
    runtime: Option<UiRuntime>,
    fs: FrameState,
    rects_key: Option<RectsKey>,
    rects: Rc<Vec<RectEntry>>,
}

impl PreviewCache {
    /// Whether the cache predates `doc_rev`/`theme_rev`.
    pub fn is_stale(&self, doc_rev: u64, theme_rev: u64) -> bool {
        self.built != Some((doc_rev, theme_rev))
    }

    /// Rebuild for a new document or theme revision.
    pub fn rebuild(
        &mut self,
        (doc_rev, theme_rev): (u64, u64),
        doc: &Document,
        theme: &Arc<Theme>,
        state: UiState,
    ) {
        self.built = Some((doc_rev, theme_rev));
        self.state = state;
        self.runtime = Some(UiRuntime::new(Arc::new(doc.clone()), theme.clone()));
        self.rects_key = None;
    }

    /// The preview state of the current revision.
    pub fn state(&self) -> &UiState {
        &self.state
    }

    /// The canvas rects of the current revision at `viewport`/`scale`,
    /// solved once per change.
    pub fn rects(&mut self, images: &DiskImages, viewport: (i32, i32), scale: i32) -> Rc<Vec<RectEntry>> {
        let (Some(built), Some(rt)) = (self.built, &self.runtime) else {
            return Rc::default();
        };
        let key = RectsKey {
            built,
            viewport,
            scale,
        };
        if self.rects_key != Some(key) {
            self.rects = Rc::new(layout_rects(
                rt.doc(),
                rt.theme(),
                &self.state,
                images,
                viewport,
                scale,
            ));
            self.rects_key = Some(key);
        }
        self.rects.clone()
    }

    /// Render the current revision (empty until the first rebuild).
    pub fn render(
        &mut self,
        images: &DiskImages,
        screen: (u32, u32),
        scale: i32,
        forced: Option<&PreviewState>,
    ) -> Vec<u8> {
        let Some(rt) = &self.runtime else {
            return Vec::new();
        };
        render_rgba(rt, &mut self.fs, &self.state, images, screen, scale, forced)
    }
}

// ---- editor-chrome geometry -------------------------------------------------------

/// One document node's solved geometry for canvas hit-testing/overlays.
pub struct RectEntry {
    pub path: NodePath,
    /// Logical px (multiply by scale for physical).
    pub rect: RectI,
    pub type_name: &'static str,
    pub abs: bool,
    pub slot_role: Option<String>,
}

/// Solve the document exactly like the runtime does (same expand + solve
/// math) and map every instance back to its document node path. List stamps
/// map to their template node, so an entry's path may repeat.
pub fn layout_rects(
    doc: &Document,
    theme: &Theme,
    state: &UiState,
    images: &DiskImages,
    viewport: (i32, i32),
    gui_scale: i32,
) -> Vec<RectEntry> {
    // Document node paths in pre-order: an instance's `node_id` indexes this.
    fn walk(n: &petramond_ui::Node, path: &mut NodePath, out: &mut Vec<NodePath>) {
        out.push(path.clone());
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            walk(c, path, out);
            path.pop();
        }
    }
    let mut paths = Vec::new();
    walk(&doc.root, &mut Vec::new(), &mut paths);

    let tree = InstTree::expand(doc, state);
    if tree.is_empty() {
        return Vec::new();
    }
    let env = ThemeEnv {
        theme,
        gui_scale,
        image_size: &|name| images.resolve(name).map(|(_, (w, h))| (w as i32, h as i32)),
    };
    let solved = petramond_ui::solve(&tree, &env, viewport, &|_| 0);
    let mut out = Vec::with_capacity(tree.len());
    for (i, inst) in tree.insts.iter().enumerate() {
        let Some(path) = paths.get(inst.node_id as usize) else {
            continue;
        };
        let slot_role = match &inst.node.kind {
            petramond_ui::NodeKind::Slot { role, .. }
            | petramond_ui::NodeKind::SlotGrid { role, .. } => Some(role.clone()),
            _ => None,
        };
        out.push(RectEntry {
            path: path.clone(),
            rect: solved.rects[i],
            type_name: inst.node.kind.type_name(),
            abs: inst.node.layout.abs.is_some(),
            slot_role,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_round_trip_export_parses_and_renders() {
        // Create → save v2 → load → export .gui.json → runtime parses,
        // validates, and rasterizes to a plausibly non-empty image.
        let p = crate::project::Project::new("petramond:pause");
        let saved = p.to_json_pretty();
        let loaded = crate::project::Project::from_json(&saved).unwrap();
        let exported = loaded.document.to_json_pretty();
        let doc = Document::from_json(&exported).unwrap();
        let contract = petramond_ui::contract::contract_for_document(&doc).unwrap();
        assert_eq!(doc.validate(None, Some(&contract)), vec![]);

        let theme = Arc::new(Theme::placeholder());
        let (state, errs) = loaded.sample_ui_state();
        assert!(errs.is_empty());
        let images = DiskImages::empty();
        let rt = UiRuntime::new(Arc::new(doc), theme);
        let mut fs = FrameState::new();
        let rgba = render_rgba(&rt, &mut fs, &state, &images, (320, 240), 1, None);
        assert_eq!(rgba.len(), 320 * 240 * 4);
        let non_clear = rgba
            .chunks_exact(4)
            .filter(|px| px[..3] != CLEAR[..3])
            .count();
        assert!(non_clear > 500, "panel pixels rendered, got {non_clear}");
    }

    #[test]
    fn layout_rects_map_back_to_document_paths() {
        let p = crate::project::Project::new("petramond:chest");
        let theme = Theme::placeholder();
        let state = UiState::new();
        let images = DiskImages::empty();
        let rects = layout_rects(&p.document, &theme, &state, &images, (400, 300), 1);
        // Root plus every child expands (no bindings hide anything).
        assert_eq!(rects.len(), 1 + p.document.root.children.len());
        assert_eq!(rects[0].path, Vec::<usize>::new());
        let storage = rects
            .iter()
            .find(|r| r.slot_role.as_deref() == Some("container"))
            .unwrap();
        let node = crate::doc_edit::node_at(&p.document.root, &storage.path).unwrap();
        assert_eq!(node.kind.type_name(), "slot_grid");
        assert!(storage.rect.w > 0 && storage.rect.h > 0);
    }

    /// A repaint with nothing changed reuses the state, the rects and the
    /// runtime's solved layout; a new revision rebuilds them.
    #[test]
    fn the_editor_cache_rebuilds_only_on_a_new_revision() {
        let p = crate::project::Project::new("petramond:chest");
        let theme = Arc::new(Theme::placeholder());
        let images = DiskImages::empty();
        let mut cache = PreviewCache::default();
        assert!(cache.is_stale(0, 0));
        cache.rebuild((1, 0), &p.document, &theme, UiState::new());
        assert!(!cache.is_stale(1, 0));
        let a = cache.rects(&images, (400, 300), 1);
        let b = cache.rects(&images, (400, 300), 1);
        assert!(Rc::ptr_eq(&a, &b), "unchanged inputs reuse the solved rects");
        let first = cache.render(&images, (400, 300), 1, None);
        let second = cache.render(&images, (400, 300), 1, None);
        assert_eq!(first, second);
        assert!(
            cache.fs.cache_stats().layout_reused,
            "the kept runtime reuses its layout between renders"
        );
        assert!(cache.is_stale(2, 0) && cache.is_stale(1, 1));
        cache.rebuild((2, 0), &p.document, &theme, UiState::new());
        let c = cache.rects(&images, (400, 300), 1);
        assert!(!Rc::ptr_eq(&a, &c), "a new revision re-solves");
    }
}
