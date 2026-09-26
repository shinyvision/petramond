//! Project file I/O: open/save `.llgui` v2 projects and export the bare
//! document to the game's `assets/ui/documents/`. rfd dialogs live here so
//! the app only deals in results.

use crate::assets::AssetRoots;
use crate::project::Project;
use petramond_ui::Document;
use std::path::{Path, PathBuf};

pub fn load_project(path: &Path) -> Result<Project, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Project::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save_project(path: &Path, project: &Project) -> Result<(), String> {
    std::fs::write(path, project.to_json_pretty())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Export the bare document as pretty `.gui.json` (what the game loads),
/// copying every referenced image that exists beside the project (in
/// `images_from`) next to the exported document so the game resolves them.
/// Returns how many images were copied.
pub fn export_document(
    path: &Path,
    doc: &Document,
    images_from: Option<&Path>,
) -> Result<usize, String> {
    std::fs::write(path, doc.to_json_pretty())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    let (Some(from), Some(to)) = (images_from, path.parent()) else {
        return Ok(0);
    };
    let mut copied = 0;
    for name in crate::doc_edit::static_image_names(doc) {
        let src = from.join(&name);
        let dst = to.join(&name);
        if !src.is_file() || src == dst {
            continue;
        }
        std::fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
        copied += 1;
    }
    Ok(copied)
}

/// "Choose image…" flow: pick a PNG, copy it beside the project file, return
/// its bare file name (what the document stores).
pub fn choose_project_image(project_dir: Option<&Path>) -> Result<Option<String>, String> {
    let Some(dir) = project_dir else {
        return Err("save the project first — images live beside the .llgui file".into());
    };
    let Some(src) = rfd::FileDialog::new()
        .add_filter("PNG image", &["png"])
        .pick_file()
    else {
        return Ok(None);
    };
    let name = src
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .ok_or("picked file has no name")?;
    let dst = dir.join(&name);
    if src != dst {
        std::fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
    }
    Ok(Some(name))
}

/// Resolve a document image the way the game will after export: beside the
/// project first, then in `ui/documents/` of the highest-priority asset layer
/// holding it — the pack the document ships in, then the base game (where
/// generated samples find the shipped images they reference).
pub fn resolve_document_image_path(
    roots: &AssetRoots,
    project_dir: Option<&Path>,
    name: &str,
) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if let Some(dir) = project_dir {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    roots.find(&format!("ui/documents/{name}"))
}

/// Every shipped `*.gui.json` as `(stem, path)`, sorted by stem.
pub fn shipped_documents(roots: &AssetRoots) -> Vec<(String, PathBuf)> {
    let Some(dir) = roots.documents_dir() else {
        return Vec::new();
    };
    files_with_suffix(&dir, ".gui.json")
}

/// What [`make_samples`] did, by stem.
pub struct SamplesMade {
    pub regenerated: Vec<String>,
    /// Samples whose document no longer ships.
    pub deleted: Vec<String>,
}

/// Regenerate `samples/<stem>.llgui` for every shipped document — the
/// document verbatim + sample_state seeded from the bindings catalog — and
/// delete every sample whose document no longer ships, so the sample list is
/// exactly the shipped set. Deterministic from doc + catalog only: hand
/// edits are NOT preserved.
pub fn make_samples(roots: &AssetRoots) -> Result<SamplesMade, String> {
    let shipped = shipped_documents(roots);
    if shipped.is_empty() {
        return Err("no shipped documents found (run inside the repo or pass --assets)".into());
    }
    let out_dir = roots
        .samples_dir()
        .ok_or("no gui-builder/samples beside the assets (run inside the repo)")?;
    let catalog = crate::bindings::Catalog::load(roots);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let mut regenerated = Vec::new();
    for (stem, path) in &shipped {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let document =
            Document::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut proj = Project {
            version: crate::project::PROJECT_VERSION,
            document,
            editor: Default::default(),
        };
        if let Some(info) = catalog.as_ref().and_then(|c| c.kind(&proj.document.kind)) {
            for (key, value) in crate::bindings::seed_values(info) {
                proj.editor
                    .sample_state
                    .insert(key, crate::project::value_to_json(&value));
            }
        }
        save_project(&out_dir.join(format!("{stem}.llgui")), &proj)?;
        regenerated.push(stem.clone());
    }
    let mut deleted = Vec::new();
    for (stem, path) in files_with_suffix(&out_dir, ".llgui") {
        if !shipped.iter().any(|(s, _)| *s == stem) {
            std::fs::remove_file(&path).map_err(|e| format!("delete {}: {e}", path.display()))?;
            deleted.push(stem);
        }
    }
    Ok(SamplesMade {
        regenerated,
        deleted,
    })
}

/// The available sample projects as `(stem, path)`, sorted.
pub fn list_samples(roots: &AssetRoots) -> Vec<(String, PathBuf)> {
    roots
        .samples_dir()
        .map(|dir| files_with_suffix(&dir, ".llgui"))
        .unwrap_or_default()
}

/// The files in `dir` named `<stem><suffix>`, as `(stem, path)` sorted by stem.
fn files_with_suffix(dir: &Path, suffix: &str) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(suffix)?.to_owned();
            Some((stem, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// Default export file name for a document kind (`petramond:pause` → `pause.gui.json`).
pub fn export_file_name(doc: &Document) -> String {
    let stem = doc.kind.split(':').next_back().unwrap_or("document");
    format!("{stem}.gui.json")
}

// ---- dialogs -----------------------------------------------------------------

fn dialog(dir: &Option<PathBuf>) -> rfd::FileDialog {
    let mut d = rfd::FileDialog::new();
    if let Some(dir) = dir {
        d = d.set_directory(dir);
    }
    d
}

pub fn pick_open(last_dir: &Option<PathBuf>) -> Option<PathBuf> {
    dialog(last_dir)
        .add_filter("GUI project", &["llgui"])
        .pick_file()
}

pub fn pick_save(last_dir: &Option<PathBuf>, name: &str) -> Option<PathBuf> {
    dialog(last_dir)
        .add_filter("GUI project", &["llgui"])
        .set_file_name(name)
        .save_file()
}

pub fn pick_export(
    doc: &Document,
    roots: &AssetRoots,
    last_dir: &Option<PathBuf>,
) -> Option<PathBuf> {
    let dir = roots.documents_dir().or_else(|| last_dir.clone());
    dialog(&dir)
        .add_filter("GUI document", &["json"])
        .set_file_name(export_file_name(doc))
        .save_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sample list is exactly the shipped document set: every shipped
    /// document has an up-to-date sample, and no sample outlives its document.
    #[test]
    fn samples_and_shipped_documents_are_the_same_set_and_up_to_date() {
        let roots = AssetRoots::new(None, Vec::new());
        let shipped = shipped_documents(&roots);
        assert!(
            !shipped.is_empty(),
            "no shipped documents under assets/ui/documents — repo layout changed?"
        );
        let shipped_stems: Vec<&str> = shipped.iter().map(|(s, _)| s.as_str()).collect();
        let samples = list_samples(&roots);
        let sample_stems: Vec<&str> = samples.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            sample_stems, shipped_stems,
            "samples/ must hold exactly one project per shipped document — \
             re-run `gui-builder --make-samples` (it deletes orphans too)"
        );
        for (stem, path) in shipped {
            let sample_path = roots
                .samples_dir()
                .expect("samples dir")
                .join(format!("{stem}.llgui"));
            let sample_text = std::fs::read_to_string(&sample_path).unwrap_or_else(|_| {
                panic!(
                    "missing sample {} — run `gui-builder --make-samples`",
                    sample_path.display()
                )
            });
            let sample =
                Project::from_json(&sample_text).unwrap_or_else(|e| panic!("sample '{stem}': {e}"));
            let shipped_doc =
                Document::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(
                sample.document, shipped_doc,
                "sample '{stem}' does not match the shipped document — \
                 re-run `gui-builder --make-samples` after editing shipped documents"
            );
            let out_dir = std::env::temp_dir().join(format!(
                "petramond-gui-builder-export-{}-{stem}",
                std::process::id()
            ));
            std::fs::create_dir_all(&out_dir).unwrap();
            let out = out_dir.join(format!("{stem}.gui.json"));
            export_document(&out, &sample.document, sample_path.parent()).unwrap();
            let exported = Document::from_json(&std::fs::read_to_string(&out).unwrap()).unwrap();
            assert_eq!(
                exported, shipped_doc,
                "exporting sample '{stem}' must reproduce the shipped document"
            );
            for image in crate::doc_edit::static_image_names(&sample.document) {
                assert!(
                    resolve_document_image_path(&roots, sample_path.parent(), &image).is_some(),
                    "sample '{stem}' image '{image}' must resolve for preview"
                );
            }
            let _ = std::fs::remove_file(out);
            let _ = std::fs::remove_dir(out_dir);
        }
    }

    #[test]
    fn document_images_resolve_beside_the_project_before_the_layers() {
        let root =
            std::env::temp_dir().join(format!("gui-builder-io-images-{}", std::process::id()));
        let base = root.join("assets");
        let project = root.join("project");
        std::fs::create_dir_all(base.join("ui/documents")).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(base.join("ui/documents/a.png"), "").unwrap();
        std::fs::write(base.join("ui/documents/b.png"), "").unwrap();
        std::fs::write(project.join("b.png"), "").unwrap();
        let roots = AssetRoots::new(Some(base.clone()), Vec::new());
        let resolve = |name: &str| resolve_document_image_path(&roots, Some(&project), name);
        assert_eq!(resolve("a.png"), Some(base.join("ui/documents/a.png")));
        assert_eq!(resolve("b.png"), Some(project.join("b.png")));
        assert_eq!(resolve("c.png"), None);
        assert_eq!(resolve(""), None);
        std::fs::remove_dir_all(&root).ok();
    }
}
