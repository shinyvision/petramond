//! The live preview pipeline: run the real llama-ui runtime over the edited
//! document and rasterize its DrawList with the software rasterizer — the
//! preview *is* the game's renderer, pixel for pixel.

use crate::doc_edit::NodePath;
use crate::project::Project;
use llama_ui::raster::TextureSet;
use llama_ui::{
    DocImages, Document, FrameArgs, FrameOutput, FrameState, ImageData, InstTree, PreviewState,
    RectI, Theme, ThemeEnv, UiRuntime, UiState,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---- document images ----------------------------------------------------------

/// PNGs referenced by `image`/`rotimage` nodes, loaded from beside the project
/// (the same rule the game uses: images live beside the exported document).
/// Missing images resolve to nothing and simply don't draw.
pub struct DiskImages {
    names: Vec<String>,
    images: Vec<ImageData>,
    /// The (doc image-name set, dir) this was loaded for, for cheap reuse.
    key: (Vec<String>, Option<PathBuf>),
}

impl DiskImages {
    pub fn empty() -> DiskImages {
        DiskImages { names: Vec::new(), images: Vec::new(), key: (Vec::new(), None) }
    }

    fn wanted(doc: &Document) -> Vec<String> {
        let mut names = Vec::new();
        doc.root.visit(&mut |n| {
            if let llama_ui::NodeKind::Image { image, .. } | llama_ui::NodeKind::Rotimage { image, .. } =
                &n.kind
            {
                if !names.contains(image) {
                    names.push(image.clone());
                }
            }
        });
        names
    }

    /// Reload if the document's image set or the search dir changed.
    pub fn refresh(&mut self, doc: &Document, dir: Option<&Path>) {
        let key = (Self::wanted(doc), dir.map(PathBuf::from));
        if key == self.key {
            return;
        }
        *self = Self::load_key(key);
    }

    pub fn load(doc: &Document, dir: Option<&Path>) -> DiskImages {
        Self::load_key((Self::wanted(doc), dir.map(PathBuf::from)))
    }

    fn load_key(key: (Vec<String>, Option<PathBuf>)) -> DiskImages {
        let mut names = Vec::new();
        let mut images = Vec::new();
        if let Some(dir) = &key.1 {
            for name in &key.0 {
                let Ok(bytes) = std::fs::read(dir.join(name)) else {
                    continue;
                };
                let Ok(img) = image::load_from_memory(&bytes) else {
                    continue;
                };
                let img = img.to_rgba8();
                let size = img.dimensions();
                names.push(name.clone());
                images.push(ImageData { rgba: img.into_raw(), size });
            }
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

/// Render one full preview frame to RGBA at `screen` physical px.
pub fn render_rgba(
    doc: &Document,
    theme: &Arc<Theme>,
    state: &UiState,
    images: &DiskImages,
    screen: (u32, u32),
    scale: i32,
    forced: Option<&PreviewState>,
) -> Vec<u8> {
    let rt = UiRuntime::new(Arc::new(doc.clone()), theme.clone());
    let mut fs = FrameState::new();
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
        &mut fs,
        &mut out,
    );
    let tex = TextureSet {
        theme_atlas: &theme.atlas,
        font: &theme.font,
        doc_images: &images.texture_refs(),
    };
    let mut rgba = Vec::new();
    llama_ui::raster::rasterize(&out.draw, &tex, screen, CLEAR, &mut rgba);
    rgba
}

/// Render a project's preview (its own screen/scale settings), for the
/// `--screenshot` CLI and tests. `catalog` seeds sample data for unset keys,
/// exactly like the editor preview.
pub fn render_project(
    project: &Project,
    theme: &Arc<Theme>,
    doc_dir: Option<&Path>,
    catalog: Option<&crate::bindings::Catalog>,
) -> (Vec<u8>, (u32, u32)) {
    let (mut state, _) = project.sample_ui_state();
    if let Some(info) = catalog.and_then(|c| c.kind(&project.document.kind)) {
        crate::bindings::apply_seeds(&mut state, info);
    }
    let images = DiskImages::load(&project.document, doc_dir);
    let screen = project.editor.screen;
    let scale = project.editor.preview_scale.clamp(1, 4) as i32;
    (render_rgba(&project.document, theme, &state, &images, screen, scale, None), screen)
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
) -> Vec<RectEntry> {
    // Pointer-identity map doc node -> path.
    let mut by_ptr: Vec<(*const llama_ui::Node, NodePath)> = Vec::new();
    fn walk(n: &llama_ui::Node, path: &mut NodePath, out: &mut Vec<(*const llama_ui::Node, NodePath)>) {
        out.push((n as *const _, path.clone()));
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            walk(c, path, out);
            path.pop();
        }
    }
    walk(&doc.root, &mut Vec::new(), &mut by_ptr);

    let tree = InstTree::expand(doc, state);
    if tree.is_empty() {
        return Vec::new();
    }
    let env = ThemeEnv {
        theme,
        image_size: &|name| images.resolve(name).map(|(_, (w, h))| (w as i32, h as i32)),
    };
    let solved = llama_ui::solve(&tree, &env, viewport, &|_| 0);
    let mut out = Vec::with_capacity(tree.len());
    for i in 0..tree.len() {
        let inst = tree.get(i as u32);
        let ptr = inst.node as *const _;
        let Some((_, path)) = by_ptr.iter().find(|(p, _)| std::ptr::eq(*p, ptr)) else {
            continue;
        };
        let slot_role = match &inst.node.kind {
            llama_ui::NodeKind::Slot { role } | llama_ui::NodeKind::SlotGrid { role, .. } => {
                Some(role.clone())
            }
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
        let p = crate::project::Project::new("llama:pause");
        let saved = p.to_json_pretty();
        let loaded = crate::project::Project::from_json(&saved).unwrap();
        let exported = loaded.document.to_json_pretty();
        let doc = Document::from_json(&exported).unwrap();
        let contract = crate::contracts::contract_for(&doc.kind);
        assert_eq!(doc.validate(None, Some(&contract)), vec![]);

        let theme = Arc::new(Theme::placeholder());
        let (state, errs) = loaded.sample_ui_state();
        assert!(errs.is_empty());
        let images = DiskImages::empty();
        let rgba = render_rgba(&doc, &theme, &state, &images, (320, 240), 1, None);
        assert_eq!(rgba.len(), 320 * 240 * 4);
        let non_clear = rgba
            .chunks_exact(4)
            .filter(|px| px[..3] != CLEAR[..3])
            .count();
        assert!(non_clear > 500, "panel pixels rendered, got {non_clear}");
    }

    #[test]
    fn layout_rects_map_back_to_document_paths() {
        let p = crate::project::Project::new("llama:chest");
        let theme = Theme::placeholder();
        let state = UiState::new();
        let images = DiskImages::empty();
        let rects = layout_rects(&p.document, &theme, &state, &images, (400, 300));
        // Root plus every child expands (no bindings hide anything).
        assert_eq!(rects.len(), 1 + p.document.root.children.len());
        assert_eq!(rects[0].path, Vec::<usize>::new());
        let storage = rects.iter().find(|r| r.slot_role.as_deref() == Some("storage")).unwrap();
        let node = crate::doc_edit::node_at(&p.document.root, &storage.path).unwrap();
        assert_eq!(node.kind.type_name(), "slot_grid");
        assert!(storage.rect.w > 0 && storage.rect.h > 0);
    }
}
