//! Where the builder reads game assets from: the base `assets/` root plus
//! pack overlays, layered the way the game's asset loader layers them — a
//! point file (the theme, a document image) comes from the highest-priority
//! layer holding it, and a catalog (item tags, binding docs) merges every
//! layer.
//!
//! Nothing is baked in at compile time. The base root is `--assets`, else
//! found by walking up from the working directory and then from the
//! executable (a repo checkout, or a release unpacked beside `assets/`).
//! Packs come from `--pack`, from a project's own `asset_roots`, and from
//! the pack a project is saved inside, in rising priority.

use std::path::{Path, PathBuf};

/// The layered asset roots one preview or validation reads through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetRoots {
    /// The base game `assets/` directory.
    base: Option<PathBuf>,
    /// Pack roots (each mirrors the `assets/` layout), lowest priority first.
    packs: Vec<PathBuf>,
}

impl AssetRoots {
    /// The roots named on the command line; with no `base`, the nearest
    /// `assets/` above the working directory or the executable.
    pub fn new(base: Option<PathBuf>, packs: Vec<PathBuf>) -> AssetRoots {
        let mut roots = AssetRoots {
            base: base.or_else(discover_base),
            packs: Vec::new(),
        };
        for pack in packs {
            roots.push_pack(pack);
        }
        roots
    }

    /// These roots as a project saved in `project_dir` sees them: its
    /// `asset_roots` (relative to the project), then the pack enclosing it,
    /// each layered above what came before.
    pub fn for_project(&self, project_dir: Option<&Path>, asset_roots: &[String]) -> AssetRoots {
        let mut roots = self.clone();
        if let Some(dir) = project_dir {
            for rel in asset_roots {
                roots.push_pack(dir.join(rel));
            }
            if let Some(pack) = enclosing_pack(dir) {
                roots.push_pack(pack);
            }
        }
        roots
    }

    /// Layer `dir` above every current pack (moving it up if already listed).
    fn push_pack(&mut self, dir: PathBuf) {
        self.packs.retain(|p| *p != dir);
        self.packs.push(dir);
    }

    #[cfg(test)]
    pub fn base(&self) -> Option<&Path> {
        self.base.as_deref()
    }

    /// Every layer directory, lowest priority first.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = &Path> {
        self.base.iter().chain(&self.packs).map(PathBuf::as_path)
    }

    /// `rel` from the highest-priority layer that has it.
    pub fn find(&self, rel: &str) -> Option<PathBuf> {
        self.layers()
            .rev()
            .map(|layer| layer.join(rel))
            .find(|path| path.is_file())
    }

    /// `rel` from every layer that has it, lowest priority first.
    pub fn all(&self, rel: &str) -> Vec<PathBuf> {
        self.layers()
            .map(|layer| layer.join(rel))
            .filter(|path| path.is_file())
            .collect()
    }

    /// The base game's document dir (`assets/ui/documents`).
    pub fn documents_dir(&self) -> Option<PathBuf> {
        let dir = self.base.as_ref()?.join("ui/documents");
        dir.is_dir().then_some(dir)
    }

    /// The builder's generated sample projects, beside the base assets in a
    /// repo checkout (`gui-builder/samples`). `None` outside a checkout.
    pub fn samples_dir(&self) -> Option<PathBuf> {
        let repo = self.base.as_ref()?.parent()?;
        let builder = repo.join("gui-builder");
        builder
            .join("Cargo.toml")
            .is_file()
            .then(|| builder.join("samples"))
    }
}

/// The nearest ancestor of `dir` holding a `pack.json`: the pack a document
/// saved there ships in.
pub fn enclosing_pack(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join("pack.json").is_file())
        .map(Path::to_path_buf)
}

/// The first `assets/` (one holding `ui/`) above the working directory,
/// else above the executable.
fn discover_base() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    [cwd, exe_dir].into_iter().flatten().find_map(|start| {
        start
            .ancestors()
            .map(|dir| dir.join("assets"))
            .find(|assets| assets.join("ui").is_dir())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("gui-builder-assets-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_pack_file_shadows_the_base_and_catalogs_see_every_layer() {
        let root = scratch("layers");
        let base = root.join("assets");
        let pack = root.join("mods/demo");
        for dir in [&base, &pack] {
            std::fs::create_dir_all(dir.join("ui/theme")).unwrap();
            std::fs::write(dir.join("ui/theme/theme.json"), "{}").unwrap();
            std::fs::write(dir.join("items.json"), "{}").unwrap();
        }
        std::fs::write(pack.join("pack.json"), r#"{ "id": "demo" }"#).unwrap();
        std::fs::write(base.join("only_base.txt"), "").unwrap();

        let roots = AssetRoots::new(Some(base.clone()), vec![pack.clone()]);
        assert_eq!(
            roots.find("ui/theme/theme.json"),
            Some(pack.join("ui/theme/theme.json"))
        );
        assert_eq!(
            roots.find("only_base.txt"),
            Some(base.join("only_base.txt"))
        );
        assert_eq!(
            roots.all("items.json"),
            vec![base.join("items.json"), pack.join("items.json")]
        );
        assert_eq!(roots.find("missing.json"), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_project_inside_a_pack_layers_that_pack_on_top() {
        let root = scratch("project");
        let base = root.join("assets");
        let other = root.join("other_pack");
        let pack = root.join("my_pack");
        let docs = pack.join("ui/documents");
        std::fs::create_dir_all(base.join("ui")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(pack.join("pack.json"), r#"{ "id": "mine" }"#).unwrap();

        let cli = AssetRoots::new(Some(base.clone()), Vec::new());
        let seen = cli.for_project(Some(&docs), &["../../../other_pack".into()]);
        let layers: Vec<&Path> = seen.layers().collect();
        assert_eq!(
            layers,
            vec![
                base.as_path(),
                docs.join("../../../other_pack").as_path(),
                pack.as_path()
            ]
        );
        assert_eq!(enclosing_pack(&docs), Some(pack.clone()));
        // A loose project adds nothing.
        assert_eq!(cli.for_project(None, &[]), cli);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_repo_checkout_is_discovered_without_compile_time_paths() {
        // Tests run from the crate dir, inside the repo.
        let roots = AssetRoots::new(None, Vec::new());
        let base = roots
            .base()
            .expect("repo assets found from the working directory");
        assert!(
            base.join("ui/theme/theme.json").is_file(),
            "{}",
            base.display()
        );
        assert!(roots.samples_dir().is_some());
    }
}
