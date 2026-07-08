//! Project file I/O: open/save `.llgui` v2 projects, import legacy v1 files,
//! and export the bare document to the game's `assets/ui/documents/`.
//! rfd dialogs live here so the app only deals in results.

use crate::legacy_import;
use crate::project::{self, Project};
use petramond_ui::Document;
use std::path::{Path, PathBuf};

pub fn load_project(path: &Path) -> Result<Project, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    Project::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save_project(path: &Path, project: &Project) -> Result<(), String> {
    std::fs::write(path, project.to_json_pretty())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Import a legacy v1 `.llgui` into a fresh v2 project.
pub fn import_legacy(path: &Path) -> Result<(Project, Vec<String>), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !project::is_legacy_json(&text) {
        return Err(format!("{} is not a legacy v1 .llgui", path.display()));
    }
    let imported = legacy_import::import(&text)?;
    let mut p = Project::new(&imported.document.kind);
    p.document = imported.document;
    Ok((p, imported.warnings))
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
    let Some(src) = rfd::FileDialog::new().add_filter("PNG image", &["png"]).pick_file() else {
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

/// The game's document dir, when the builder runs inside the repo.
pub fn game_documents_dir() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?;
    let dir = repo.join("assets/ui/documents");
    dir.is_dir().then_some(dir)
}

/// Resolve a document image the way the game will after export. Normal
/// projects resolve beside the `.llgui`; generated samples may reference paths
/// that are correct beside `assets/ui/documents`, so fall back there for
/// preview/validation without rewriting the document.
pub fn resolve_document_image_path(project_dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if let Some(dir) = project_dir {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = game_documents_dir()?.join(name);
    path.is_file().then_some(path)
}

/// The builder's sample-project dir (`gui-builder/samples/`).
pub fn samples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("samples")
}

/// Every shipped `*.gui.json` as `(stem, path)`, sorted by stem.
pub fn shipped_documents() -> Vec<(String, PathBuf)> {
    let Some(dir) = game_documents_dir() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".gui.json")?.to_owned();
            Some((stem, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// Regenerate `samples/<stem>.llgui` for every shipped document: the document
/// verbatim + sample_state seeded from the bindings catalog. Deterministic
/// from doc + catalog only — hand edits are NOT preserved.
pub fn make_samples() -> Result<Vec<String>, String> {
    let shipped = shipped_documents();
    if shipped.is_empty() {
        return Err("no shipped documents found (run inside the repo)".into());
    }
    let catalog = crate::bindings::Catalog::load();
    let out_dir = samples_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let mut names = Vec::new();
    for (stem, path) in shipped {
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let document = Document::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))?;
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
        names.push(stem);
    }
    Ok(names)
}

/// The available sample projects as `(stem, path)`, sorted.
pub fn list_samples() -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(samples_dir()) else { return Vec::new() };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".llgui")?.to_owned();
            Some((stem, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// Default export file name for a document kind (`petramond:pause` → `pause.gui.json`).
pub fn export_file_name(doc: &Document) -> String {
    let stem = doc.kind.split(':').last().unwrap_or("document");
    format!("{stem}.gui.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_document_has_an_up_to_date_sample() {
        let shipped = shipped_documents();
        assert!(
            !shipped.is_empty(),
            "no shipped documents under assets/ui/documents — repo layout changed?"
        );
        for (stem, path) in shipped {
            let sample_path = samples_dir().join(format!("{stem}.llgui"));
            let sample_text = std::fs::read_to_string(&sample_path).unwrap_or_else(|_| {
                panic!(
                    "missing sample {} — run `gui-builder --make-samples`",
                    sample_path.display()
                )
            });
            let sample = Project::from_json(&sample_text)
                .unwrap_or_else(|e| panic!("sample '{stem}': {e}"));
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
                if image.is_empty() {
                    continue;
                }
                assert!(
                    resolve_document_image_path(sample_path.parent(), &image).is_some(),
                    "sample '{stem}' image '{image}' must resolve for preview"
                );
            }
            let _ = std::fs::remove_file(out);
            let _ = std::fs::remove_dir(out_dir);
        }
    }
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
    dialog(last_dir).add_filter("GUI project", &["llgui"]).pick_file()
}

pub fn pick_save(last_dir: &Option<PathBuf>, name: &str) -> Option<PathBuf> {
    dialog(last_dir)
        .add_filter("GUI project", &["llgui"])
        .set_file_name(name)
        .save_file()
}

pub fn pick_export(doc: &Document, last_dir: &Option<PathBuf>) -> Option<PathBuf> {
    let dir = game_documents_dir().or_else(|| last_dir.clone());
    dialog(&dir)
        .add_filter("GUI document", &["json"])
        .set_file_name(&export_file_name(doc))
        .save_file()
}
