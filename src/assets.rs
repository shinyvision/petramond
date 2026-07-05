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
//! A pack is a directory under a `mods/` root containing a `pack.json`
//! manifest. Roots, in priority order (first root providing a pack directory
//! NAME wins for that pack):
//!
//! 1. the `LLAMACRAFT_MODS` env override — REPLACES every other root
//!    (tests / explicit launches mean exactly that mod set),
//! 2. `mods/` under the working directory (the dev tree),
//! 3. `<OS data dir>/llamacraft/mods` (e.g. `~/.local/share/llamacraft/mods`)
//!    — where players install packs without touching the install,
//! 4. `mods/` alongside the executable (packs shipped with the game).
//!
//! The manifest:
//!
//! ```json
//! {
//!   "name": "My Pack",
//!   "id": "mypack",
//!   "version": "0.1.0",
//!   "description": "...",
//!   "wasm": "mod.wasm",
//!   "dependencies": ["othermod"],
//!   "after": ["thirdmod"]
//! }
//! ```
//!
//! Only `name` is required. `id` is the pack's stable snake_case namespace
//! (except reserved `llama`, which belongs to the engine) — required as soon as
//! the pack ships `wasm` or introduces namespaced (`id:name`) catalog keys, and
//! every namespaced key the pack states must carry ITS OWN id as the prefix (a
//! violation disables the whole pack with a logged error — packs never load
//! partially).
//!
//! Load order = topological sort by `dependencies` + `after`, ties broken by
//! directory name (so unconstrained packs keep the classic `10_terrain`,
//! `20_sounds` prefix-name ordering); a missing dependency disables the pack
//! and, transitively, its dependents. See `crate::modding::manifest`.
//!
//! This order feeds dynamic registry id assignment (`crate::registry`): ids
//! are handed out in pack load order past the engine range. Editing
//! `dependencies`/`after` (or renaming pack directories) may therefore
//! renumber dynamic ids between sessions — that is SAFE for saves, because
//! `save/palette.json` addresses content by NAME and remaps ids on load; only
//! within-session numeric ids move.
//!
//! Its files mirror the `assets/` layout, later packs winning. Two resolution
//! modes:
//!
//! - **Point files** ([`read_bytes`]: textures, models,
//!   sounds): the highest-priority pack that has the file wins; base `assets/`
//!   is the fallback. Overriding one texture = shipping just that file.
//! - **Layered catalogs** ([`read_layers`]: `blocks.json`, `items.json`,
//!   `recipes.json`, `loot_tables.json`, `textures/atlas.json`, `shaders.json`):
//!   EVERY copy is
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

/// A discovered, validated mod pack in load order.
pub struct Pack {
    pub dir: PathBuf,
    /// The pack's display name (`pack.json` `name` — the only required field).
    pub name: String,
    /// The pack's namespace id (`None` = content-only point-file override pack).
    pub id: Option<String>,
    /// The pack's declared version string, for the save's mod-set record
    /// (`mods.json` — see `modding::modset`).
    pub version: Option<String>,
    /// Human-readable description from `pack.json`, used by shell presentation.
    pub description: String,
    /// Short row copy for compact shell lists. Falls back to `description`.
    pub summary: Option<String>,
    /// Absolute path of the pack's icon PNG (for mod lists), when it ships one.
    pub icon: Option<PathBuf>,
    /// Absolute path of the pack's compiled logic, when it ships one.
    pub wasm: Option<PathBuf>,
}

/// Discovered packs in LOAD order (lowest priority first — the merge order for
/// layered catalogs; point files search the reverse).
pub fn packs() -> &'static [Pack] {
    static PACKS: LazyLock<Vec<Pack>> = LazyLock::new(discover_packs);
    &PACKS
}

/// `mods/` directories searched for packs, in priority order (see the module
/// docs): dev tree, then the user's OS data dir, then alongside the
/// executable. Unlike the additive base roots, the `LLAMACRAFT_MODS` override
/// REPLACES the default roots: pointing the game (or a test child process) at
/// a mods dir must mean exactly that mod set, not "that plus whatever the
/// working directory carries".
fn mod_roots() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("LLAMACRAFT_MODS") {
        return vec![PathBuf::from(dir)];
    }
    let mut roots = vec![PathBuf::from("mods")];
    roots.push(crate::save::base_data_dir().join("mods"));
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
    id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    summary: Option<String>,
    /// Pack-relative path of the pack's icon PNG, if any.
    #[serde(default)]
    icon: Option<String>,
    /// Pack-relative path of the compiled mod logic, if any.
    #[serde(default)]
    wasm: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    after: Vec<String>,
}

fn discover_packs() -> Vec<Pack> {
    use crate::modding::manifest::{self, PackMeta};

    // Gather candidates: the FIRST root providing a pack directory name wins
    // (mirrors the base-root priority), so a dev-tree pack shadows an
    // installed one. Sorted by directory name = the deterministic input order.
    let mut found: Vec<(String, PathBuf, PackManifest)> = Vec::new();
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
            if found.iter().any(|(n, _, _)| *n == dir_name) {
                continue;
            }
            let manifest = dir.join("pack.json");
            let Ok(text) = std::fs::read_to_string(&manifest) else {
                continue; // not a pack (no manifest) — ignore silently
            };
            match serde_json::from_str::<PackManifest>(&text) {
                Ok(m) => found.push((dir_name, dir, m)),
                Err(e) => log::error!("ignoring pack {}: bad pack.json: {e}", manifest.display()),
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));

    // Per-pack validation that needs the pack's files: the wasm file must
    // exist, and every namespaced catalog key must carry the pack's own id.
    // A violating pack is disabled whole — never a partial load.
    found.retain(|(dir_name, dir, m)| {
        let disable = |why: &str| {
            log::error!("mod pack '{dir_name}' disabled: {why}");
            false
        };
        if let Some(wasm) = &m.wasm {
            if !dir.join(wasm).is_file() {
                return disable(&format!("declared wasm '{wasm}' not found in the pack"));
            }
        }
        let keys = match manifest::registration_keys(dir) {
            Ok(keys) => keys,
            Err(e) => return disable(&e),
        };
        let foreign = manifest::foreign_namespaced_keys(m.id.as_deref(), &keys);
        if !foreign.is_empty() {
            return disable(&format!(
                "namespaced catalog keys must use the pack's own id ('{}:'): {}",
                m.id.as_deref().unwrap_or("<no id>"),
                foreign.join(", ")
            ));
        }
        true
    });

    // Load-order resolution: manifest validity, dependency cascade, topo sort.
    let metas: Vec<PackMeta> = found
        .iter()
        .map(|(dir_name, _, m)| PackMeta {
            dir_name: dir_name.clone(),
            id: m.id.clone(),
            wasm: m.wasm.is_some(),
            dependencies: m.dependencies.clone(),
            after: m.after.clone(),
        })
        .collect();
    let order = manifest::resolve_load_order(&metas, |i, why| {
        log::error!("mod pack '{}' disabled: {why}", metas[i].dir_name);
    });

    order
        .into_iter()
        .map(|i| {
            let (_, dir, m) = &found[i];
            log::info!("mod pack '{}' loaded from {}", m.name, dir.display());
            Pack {
                dir: dir.clone(),
                name: m.name.clone(),
                id: m.id.clone(),
                version: m.version.clone(),
                description: m.description.clone(),
                summary: m.summary.clone(),
                icon: m
                    .icon
                    .as_ref()
                    .map(|i| dir.join(i))
                    .filter(|p| p.is_file()),
                wasm: m.wasm.as_ref().map(|w| dir.join(w)),
            }
        })
        .collect()
}

/// Candidate absolute paths for the asset at `rel` (e.g. `recipes.json`), in
/// priority order: packs (highest priority first), then the base roots.
pub fn candidate_paths(rel: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = packs().iter().rev().map(|p| p.dir.join(rel)).collect();
    paths.extend(base_roots().into_iter().map(|r| r.join(rel)));
    paths
}

/// Read the shipped BASE copy of `rel` — packs deliberately excluded — with
/// the path it loaded from, or `None` if no base root has it. This is the
/// loaders' shipped-file test gate: "the base catalog is valid on its own"
/// must not change meaning because a mod pack happens to be installed.
/// (Runtime catalogues read [`read_layers`]; runtime point files read
/// [`read_bytes`].)
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_base_text(rel: &str) -> Option<(String, PathBuf)> {
    for root in base_roots() {
        let path = root.join(rel);
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
/// Each directory carries its owning pack namespace id (`None` for base dirs
/// and id-less override packs) so loaders can validate namespaced content
/// against the pack that ships it (mod GUI kinds).
pub fn layer_dirs_with_ids(rel: &str) -> Vec<(PathBuf, Option<String>)> {
    let mut out: Vec<(PathBuf, Option<String>)> = base_roots()
        .into_iter()
        .rev()
        .map(|r| (r.join(rel), None))
        .collect();
    out.extend(packs().iter().map(|p| (p.dir.join(rel), p.id.clone())));
    out.retain(|(p, _)| p.is_dir());
    out
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
