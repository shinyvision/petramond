//! Locating on-disk data files at runtime, with mod-pack overlays.
//!
//! Data-driven content (block/item defs, recipes, loot tables, textures,
//! models) lives under `assets/`; this module finds that directory wherever
//! the game runs from, so every loader resolves files the same way. Base
//! candidate roots, in priority order: the `LLAMACRAFT_ASSETS` env override,
//! `assets/` under the working directory (the dev tree), then `assets/` (or
//! the bare file) alongside the executable (a shipped install).
//!
//! # Mod packs
//!
//! A pack is a directory under `mods/` (beside `assets/`, or at the
//! `LLAMACRAFT_MODS` env override) containing a `pack.json` manifest:
//!
//! ```json
//! { "name": "My Pack", "description": "..." }
//! ```
//!
//! Its files mirror the `assets/` layout. Packs load in directory-name order,
//! later packs winning (prefix names like `10_terrain`, `20_sounds` to order
//! them). Two resolution modes:
//!
//! - **Point files** ([`read_text`] / [`read_bytes`]: textures, models,
//!   sounds): the highest-priority pack that has the file wins; base `assets/`
//!   is the fallback. Overriding one texture = shipping just that file.
//! - **Layered catalogs** ([`read_layers`]: `blocks.json`, `items.json`,
//!   `recipes.json`, `loot_tables.json`, `textures/atlas.json`): EVERY copy is
//!   returned base-first and the caller merges — by entry key (later packs
//!   replace or extend) or by appending (recipes) — so a pack states only what
//!   it changes, never a full copy of the catalogue.

use std::path::PathBuf;
use std::sync::LazyLock;

/// Base asset directories (no packs), in priority order (first wins).
fn base_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("LLAMACRAFT_ASSETS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from("assets"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("assets"));
            roots.push(dir.to_path_buf());
        }
    }
    roots
}

/// A discovered mod pack (its asset root; the manifest name is logged at
/// discovery).
pub struct Pack {
    pub dir: PathBuf,
}

/// Discovered packs in LOAD order (lowest priority first — the merge order for
/// layered catalogs; point files search the reverse).
pub fn packs() -> &'static [Pack] {
    static PACKS: LazyLock<Vec<Pack>> = LazyLock::new(discover_packs);
    &PACKS
}

/// `mods/` directories searched for packs, mirroring the base-root candidates.
fn mod_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("LLAMACRAFT_MODS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from("mods"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("mods"));
        }
    }
    roots
}

#[derive(serde::Deserialize)]
struct PackManifest {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
}

fn discover_packs() -> Vec<Pack> {
    let mut found: Vec<(String, Pack)> = Vec::new();
    for root in mod_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            // The FIRST root providing a pack directory name wins (mirrors the
            // base-root priority), so a dev-tree pack shadows an installed one.
            if found.iter().any(|(n, _)| *n == dir_name) {
                continue;
            }
            let manifest = dir.join("pack.json");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue; // not a pack (no manifest) — ignore silently
            };
            match serde_json::from_str::<PackManifest>(&text) {
                Ok(m) => {
                    log::info!("mod pack '{}' loaded from {}", m.name, dir.display());
                    found.push((dir_name, Pack { dir }));
                }
                Err(e) => log::error!("ignoring pack {}: bad pack.json: {e}", manifest.display()),
            }
        }
    }
    // Load order = directory-name order (prefix names to control priority).
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Candidate absolute paths for the asset at `rel` (e.g. `recipes.json`), in
/// priority order: packs (highest priority first), then the base roots.
pub fn candidate_paths(rel: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = packs().iter().rev().map(|p| p.dir.join(rel)).collect();
    paths.extend(base_roots().into_iter().map(|r| r.join(rel)));
    paths
}

/// Read the first readable candidate for `rel` (point-file resolution: the
/// highest-priority pack wins, base `assets/` is the fallback), returning its
/// text and the path it loaded from, or `None` if no candidate exists.
/// (Runtime text catalogues read [`read_layers`] instead; this is the point-file
/// text form, used by the loaders' shipped-file test gates.)
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_text(rel: &str) -> Option<(String, PathBuf)> {
    for path in candidate_paths(rel) {
        if let Ok(s) = std::fs::read_to_string(&path) {
            return Some((s, path));
        }
    }
    None
}

/// Read the first readable candidate for `rel` as raw bytes (textures, models,
/// sounds), or `None` if no candidate exists.
pub fn read_bytes(rel: &str) -> Option<(Vec<u8>, PathBuf)> {
    for path in candidate_paths(rel) {
        if let Ok(b) = std::fs::read(&path) {
            return Some((b, path));
        }
    }
    None
}

/// Existing directories for `rel` across the base roots + packs, LOWEST
/// priority first — callers overlay their contents by filename, later dirs
/// winning (e.g. a pack's baked GUI shadows the base one of the same name).
pub fn layer_dirs(rel: &str) -> Vec<PathBuf> {
    candidate_paths(rel)
        .into_iter()
        .rev()
        .filter(|p| p.is_dir())
        .collect()
}

/// Read EVERY copy of the layered catalog `rel`, lowest priority first: the
/// base file (from the first base root that has it), then each pack's copy in
/// load order. The caller merges layers by its catalogue's key semantics.
/// Empty if nothing provides the file.
pub fn read_layers(rel: &str) -> Vec<(String, PathBuf)> {
    let mut layers = Vec::new();
    for root in base_roots() {
        let path = root.join(rel);
        if let Ok(s) = std::fs::read_to_string(&path) {
            layers.push((s, path));
            break; // base roots shadow each other; only one base layer
        }
    }
    for pack in packs() {
        let path = pack.dir.join(rel);
        if let Ok(s) = std::fs::read_to_string(&path) {
            layers.push((s, path));
        }
    }
    layers
}
