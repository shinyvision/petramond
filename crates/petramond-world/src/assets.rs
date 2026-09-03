//! Locating on-disk data files at runtime, with mod-pack overlays.
//!
//! Data-driven content (block/item defs, recipes, loot tables, textures,
//! models) lives under `assets/`; this module finds that directory wherever
//! the game runs from, so every loader resolves files the same way. Base
//! candidate roots, in priority order: the `PETRAMOND_ASSETS` env override,
//! `assets/` under the working directory (the dev tree), then `assets/` (or
//! the bare file) alongside the executable (a shipped install).
//!
//! # Mod packs
//!
//! A pack is a directory under a `mods/` root containing a `pack.json`
//! manifest. Roots, in priority order (first root providing a pack directory
//! NAME wins for that pack):
//!
//! 1. the `PETRAMOND_MODS` env override — REPLACES every other root
//!    (tests / explicit launches mean exactly that mod set),
//! 2. `mods/` under the working directory (the dev tree),
//! 3. `<OS data dir>/petramond/mods` (e.g. `~/.local/share/petramond/mods`)
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
//!   "client_wasm": "client.wasm",
//!   "dependencies": ["othermod"],
//!   "after": ["thirdmod"]
//! }
//! ```
//!
//! Only `name` is required. `id` is the pack's stable snake_case namespace
//! (except reserved `petramond`, which belongs to the engine) — required as soon as
//! the pack ships `wasm` or introduces namespaced (`id:name`) catalog keys, and
//! every namespaced key the pack states must carry ITS OWN id as the prefix (a
//! violation disables the whole pack with a logged error — packs never load
//! partially).
//!
//! Load order = topological sort by `dependencies` + `after`, ties broken by
//! directory name (so unconstrained packs keep the classic `10_terrain`,
//! `20_sounds` prefix-name ordering); a missing dependency disables the pack
//! and, transitively, its dependents. See `crate::pack_manifest`.
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
//!
//! # Integrations
//!
//! A pack may ship `integrations/<mod id>/` directories, each laid out like
//! the pack itself (catalogs, `textures/`, `ui/documents/`). Such a directory
//! is an overlay of its own that joins the load ONLY while `<mod id>` names an
//! installed, enabled pack — content the shipping pack states in another
//! pack's vocabulary (its moulds for a forge, its dishes for a kitchen), with
//! the rows simply absent when the other pack is not there. Its keys still
//! carry the SHIPPING pack's id (the same ownership rule), it counts against
//! the same id budget, and it merges after every plain pack layer, because an
//! integration is written knowing both packs and is therefore the most
//! specific statement in the overlay. See [`layers`].

use std::path::PathBuf;
use std::sync::LazyLock;

/// Base asset directories (no packs), in priority order (first wins).
fn base_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("PETRAMOND_ASSETS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from("assets"));
    // The workspace checkout's assets, resolved at COMPILE time — so dev/test
    // binaries of every workspace crate find them no matter which package dir
    // cargo runs them from. Shipped builds keep resolving exe-relative below.
    // The workspace root's assets/ — CARGO_MANIFEST_DIR is this crate's dir
    // (crates/petramond-world), two levels below the repo.
    roots.push(workspace_root().join("assets"));
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
    /// Optional presentation-only client module. It runs in a separate
    /// restricted instance and cannot mutate the deterministic simulation.
    pub client_wasm: Option<PathBuf>,
    /// The pack's `integrations/<mod id>/` overlays whose target pack is
    /// installed, by target name. Ones naming an absent pack are dropped at
    /// discovery with a logged note.
    pub integrations: Vec<Integration>,
}

/// One `integrations/<target>/` directory of a pack (see the module docs).
pub struct Integration {
    /// The mod id the overlay is written against.
    pub target: String,
    pub dir: PathBuf,
}

/// One content overlay directory in merge order: a pack, or one pack's
/// integration with another.
pub struct Layer {
    pub dir: PathBuf,
    /// The pack whose namespace the layer's keys carry (`None` for an id-less
    /// override pack).
    pub owner: Option<String>,
    /// Every mod id the layer's content presumes: the owner, plus the target
    /// for an integration. A per-world disable of ANY of them must take the
    /// layer's session-scoped content (recipes) with it — an integration's
    /// patch rows retire the owner's own routes in favour of the target's,
    /// which is exactly wrong once the target is switched off.
    pub requires: Vec<String>,
}

/// Every overlay in merge order: packs in load order, then their integrations
/// in the same order. Base `assets/` is not a layer here; the readers below
/// put it first.
pub fn layers() -> &'static [Layer] {
    static LAYERS: LazyLock<Vec<Layer>> = LazyLock::new(|| {
        let mut out: Vec<Layer> = packs()
            .iter()
            .map(|p| Layer {
                dir: p.dir.clone(),
                owner: p.id.clone(),
                requires: p.id.iter().cloned().collect(),
            })
            .collect();
        for pack in packs() {
            for integration in &pack.integrations {
                let mut requires: Vec<String> = pack.id.iter().cloned().collect();
                requires.push(integration.target.clone());
                out.push(Layer {
                    dir: integration.dir.clone(),
                    owner: pack.id.clone(),
                    requires,
                });
            }
        }
        out
    });
    &LAYERS
}

/// The `integrations/<name>/` subdirectories under a pack dir, by name.
fn integration_dirs(dir: &std::path::Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir.join("integrations")) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    out.sort();
    out
}

/// The directories whose catalogs a pack contributes given the installed id
/// set: its own, plus each integration whose target is installed.
fn catalog_dirs(
    dir: &std::path::Path,
    installed: &std::collections::BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(
        integration_dirs(dir)
            .into_iter()
            .filter(|(target, _)| installed.contains(target))
            .map(|(_, sub)| sub),
    );
    dirs
}

/// Discovered packs in LOAD order (lowest priority first — the merge order for
/// layered catalogs; point files search the reverse).
pub fn packs() -> &'static [Pack] {
    static PACKS: LazyLock<Vec<Pack>> = LazyLock::new(discover_packs);
    &PACKS
}

/// `mods/` directories searched for packs, in priority order (see the module
/// docs): dev tree, then the user's OS data dir, then alongside the
/// executable. Unlike the additive base roots, the `PETRAMOND_MODS` override
/// REPLACES the default roots: pointing the game (or a test child process) at
/// a mods dir must mean exactly that mod set, not "that plus whatever the
/// working directory carries".
fn mod_roots() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("PETRAMOND_MODS") {
        return vec![PathBuf::from(dir)];
    }
    let mut roots = vec![PathBuf::from("mods")];
    // The workspace checkout's mods, compile-time-resolved like `base_roots`'
    // assets entry (dev/test binaries of sibling workspace crates).
    roots.push(workspace_root().join("mods"));
    roots.push(petramond_util::paths::base_data_dir().join("mods"));
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
    /// Pack-relative path of presentation-only client logic, if any.
    #[serde(default)]
    client_wasm: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    after: Vec<String>,
}

fn discover_packs() -> Vec<Pack> {
    use crate::pack_manifest::{self as manifest, PackMeta};

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
        if let Some(wasm) = &m.client_wasm {
            if !dir.join(wasm).is_file() {
                return disable(&format!(
                    "declared client_wasm '{wasm}' not found in the pack"
                ));
            }
        }
        let mut keys = match manifest::registration_keys(dir) {
            Ok(keys) => keys,
            Err(e) => return disable(&e),
        };
        // An integration's rows are the pack's own statements, so they obey
        // the pack's namespace whether or not the target is installed.
        for (target, sub) in integration_dirs(dir) {
            match manifest::registration_keys(&sub) {
                Ok(more) => keys.extend(more),
                Err(e) => return disable(&format!("integrations/{target}: {e}")),
            }
        }
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
            wasm: m.wasm.is_some() || m.client_wasm.is_some(),
            dependencies: m.dependencies.clone(),
            after: m.after.clone(),
        })
        .collect();
    let order = manifest::resolve_load_order(&metas, |i, why| {
        log::error!("mod pack '{}' disabled: {why}", metas[i].dir_name);
    });
    let order = enforce_id_budget(&found, &metas, order);
    let installed: std::collections::BTreeSet<String> = order
        .iter()
        .filter_map(|&i| found[i].2.id.clone())
        .collect();

    order
        .into_iter()
        .map(|i| {
            let (_, dir, m) = &found[i];
            log::info!("mod pack '{}' loaded from {}", m.name, dir.display());
            let integrations = integration_dirs(dir)
                .into_iter()
                .filter_map(|(target, sub)| {
                    if m.id.as_deref() == Some(target.as_str()) {
                        log::error!(
                            "mod pack '{}' ignores integrations/{target}: a pack cannot integrate with itself",
                            m.name
                        );
                        return None;
                    }
                    if !installed.contains(&target) {
                        log::info!(
                            "mod pack '{}' integration '{target}' skipped: that mod is not installed",
                            m.name
                        );
                        return None;
                    }
                    log::info!("mod pack '{}' integrates with '{target}'", m.name);
                    Some(Integration { target, dir: sub })
                })
                .collect();
            Pack {
                dir: dir.clone(),
                name: m.name.clone(),
                id: m.id.clone(),
                version: m.version.clone(),
                description: m.description.clone(),
                summary: m.summary.clone(),
                icon: m.icon.as_ref().map(|i| dir.join(i)).filter(|p| p.is_file()),
                wasm: m.wasm.as_ref().map(|w| dir.join(w)),
                client_wasm: m.client_wasm.as_ref().map(|w| dir.join(w)),
                integrations,
            }
        })
        .collect()
}

/// Drop, from the resolved load order, any pack whose block/item rows would
/// push a shared registry past its id ceiling — and re-run order resolution so
/// a dropped pack's dependents go with it.
///
/// The ceiling is real (ids are save- and wire-relevant, so the width is a
/// format decision, not a local one) but it is no longer tight: `Block` and
/// `ItemType` are `u16`. What this rule guarantees is that reaching it is an
/// ADMISSION outcome — the offending pack is disabled, loudly, and everything
/// before it still runs — rather than a panic inside the shared registry
/// bootstrap long after admission, which used to brick the game and name no
/// pack.
fn enforce_id_budget(
    found: &[(String, PathBuf, PackManifest)],
    metas: &[crate::pack_manifest::PackMeta],
    order: Vec<usize>,
) -> Vec<usize> {
    use crate::pack_manifest::{self as manifest, ID_CAP, ID_CAPPED_CATALOGS};

    // One catalog read per pack, reused for both capped catalogs. Admission
    // already parsed these files; a second read here keeps the budget rule
    // where the rest of the load-order policy lives. An integration's rows
    // cost the SHIPPING pack, and are costed against the packs found rather
    // than the final order — a target dropped below simply leaves the
    // estimate slightly generous.
    let installed: std::collections::BTreeSet<String> =
        found.iter().filter_map(|(_, _, m)| m.id.clone()).collect();
    let per_pack: Vec<Vec<(&'static str, Vec<String>)>> = order
        .iter()
        .map(|&i| {
            let mut merged: Vec<(&'static str, Vec<String>)> = Vec::new();
            for dir in catalog_dirs(&found[i].1, &installed) {
                for (rel, keys) in manifest::registration_keys_by_catalog(&dir).unwrap_or_default()
                {
                    match merged.iter_mut().find(|(r, _)| *r == rel) {
                        Some((_, known)) => known.extend(keys),
                        None => merged.push((rel, keys)),
                    }
                }
            }
            merged
        })
        .collect();

    let mut dropped: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for (rel, engine_names) in [
        (
            ID_CAPPED_CATALOGS[0],
            crate::block::ENGINE_BLOCK_NAMES as &[&str],
        ),
        (ID_CAPPED_CATALOGS[1], crate::item::ENGINE_ITEM_NAMES),
    ] {
        let costs: Vec<Vec<String>> = per_pack
            .iter()
            .map(|catalogs| {
                catalogs
                    .iter()
                    .find(|(r, _)| *r == rel)
                    .map(|(_, keys)| keys.clone())
                    .unwrap_or_default()
            })
            .collect();
        for (slot, would_be) in manifest::id_budget_overflow(engine_names, &costs) {
            log::error!(
                "mod pack '{}' disabled: its {rel} rows would register {would_be} names, but \
                 the registry caps at {ID_CAP}",
                metas[order[slot]].dir_name
            );
            dropped.insert(order[slot]);
        }
    }
    if dropped.is_empty() {
        return order;
    }
    // Re-resolve so the dependency cascade takes the dropped packs' dependents
    // with them, instead of leaving a pack running against content that is no
    // longer there.
    let survivors: Vec<usize> = order
        .iter()
        .copied()
        .filter(|i| !dropped.contains(i))
        .collect();
    let sub: Vec<crate::pack_manifest::PackMeta> = survivors
        .iter()
        .map(|&i| crate::pack_manifest::PackMeta {
            dir_name: metas[i].dir_name.clone(),
            id: metas[i].id.clone(),
            wasm: metas[i].wasm,
            dependencies: metas[i].dependencies.clone(),
            after: metas[i].after.clone(),
        })
        .collect();
    manifest::resolve_load_order(&sub, |j, why| {
        log::error!("mod pack '{}' disabled: {why}", sub[j].dir_name);
    })
    .into_iter()
    .map(|j| survivors[j])
    .collect()
}

/// The workspace root, from this crate's compiled-in manifest dir. Dev builds
/// run from anywhere inside the repo; installed builds never hit these roots
/// (the data-dir roots above resolve first).
fn workspace_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/petramond-world sits two levels below the workspace root")
        .to_path_buf()
}

/// Candidate absolute paths for the asset at `rel` (e.g. `recipes.json`), in
/// priority order: packs (highest priority first), then the base roots.
pub fn candidate_paths(rel: &str) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = layers().iter().rev().map(|l| l.dir.join(rel)).collect();
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
    out.extend(layers().iter().map(|l| (l.dir.join(rel), l.owner.clone())));
    out.retain(|(p, _)| p.is_dir());
    out
}

/// One copy of a layered catalog with the overlay it came from.
pub struct CatalogLayer {
    pub text: String,
    pub path: PathBuf,
    /// The owning pack namespace (`None` for the base catalog or an id-less
    /// override pack).
    pub owner: Option<String>,
    /// The mod ids the layer presumes ([`Layer::requires`]; empty for base).
    pub requires: Vec<String>,
}

/// [`read_layers`] with each layer's overlay identity. Recipe loading needs
/// it so disabling a pack removes even rows that reference engine content
/// only, and an integration's rows go with EITHER of its packs.
pub fn read_catalog_layers(rel: &str) -> Vec<CatalogLayer> {
    let mut out = Vec::new();
    for root in base_roots() {
        let path = root.join(rel);
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push(CatalogLayer {
                text,
                path,
                owner: None,
                requires: Vec::new(),
            });
            break; // base roots shadow each other; only one base layer
        }
    }
    for layer in layers() {
        let path = layer.dir.join(rel);
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push(CatalogLayer {
                text,
                path,
                owner: layer.owner.clone(),
                requires: layer.requires.clone(),
            });
        }
    }
    out
}

/// Read EVERY copy of the layered catalog `rel`, lowest priority first: the
/// base file (from the first base root that has it), then each pack's copy in
/// load order. The caller merges layers by its catalogue's key semantics.
/// Empty if nothing provides the file.
pub fn read_layers(rel: &str) -> Vec<(String, PathBuf)> {
    read_catalog_layers(rel)
        .into_iter()
        .map(|layer| (layer.text, layer.path))
        .collect()
}
