//! Finds the `assets/` dir at runtime, wherever the game runs from, plus mod-pack overlays.
//!
//! Asset roots in priority order: the `PETRAMOND_ASSETS` env override, `assets/` under the
//! working directory (dev tree), then `assets/` (or the bare file) next to the executable
//! (shipped install) or in a macOS app bundle's `Contents/Resources`.
//!
//! # Mod packs
//!
//! A pack is a visible directory under a `mods/` root with a `pack.json` manifest. Roots
//! ([`PackRoots`]): `PETRAMOND_MODS` replaces every other root and counts as shipped. Otherwise
//! the workspace `mods/` (checkout only) and `mods/` beside the executable are shipped, and
//! `<OS data dir>/petramond/mods` is the one installed root. Shipped beats installed on any
//! name or id collision; among installed packs, directory order breaks ties.
//!
//! Manifest:
//!
//! ```json
//! {
//!   "name": "My Pack",
//!   "id": "mypack",
//!   "version": "0.1.0",
//!   "description": "...",
//!   "wasm": "mod.wasm",
//!   "client_wasm": "client.wasm",
//!   "launch": { "label": "My Tool", "icon": "launch.png" },
//!   "dependencies": ["othermod"],
//!   "after": ["thirdmod"]
//! }
//! ```
//!
//! Only `name` is required. `id` is the pack's stable snake_case namespace (`petramond` is
//! reserved for the engine). It is required once the pack ships `wasm` or namespaced (`id:name`)
//! catalog keys. Every namespaced key must use the pack's own id as its prefix. A violation
//! disables the whole pack with a logged error; packs never load partially.
//!
//! Load order is a topological sort on `dependencies` + `after`, with ties broken by directory
//! name, so unconstrained packs keep the `10_terrain`, `20_sounds` prefix ordering. A missing
//! dependency disables the pack and everything depending on it. See `crate::pack_manifest`.
//!
//! This order also drives dynamic registry ids (`crate::registry`), handed out in load order
//! past the engine range. Editing `dependencies`/`after`, or renaming a pack dir, can renumber
//! dynamic ids between sessions. Saves are safe because `save/palette.json` addresses content
//! by name and remaps ids on load. Only in-session numeric ids move.
//!
//! Pack files mirror the `assets/` layout, with later packs winning. Two resolution modes:
//!
//! - Point files ([`read_bytes`]: textures, models, sounds): the highest-priority pack with the
//!   file wins, and base `assets/` is the fallback. Overriding one texture means shipping just
//!   that file.
//! - Layered catalogs ([`read_layers`]: `blocks.json`, `items.json`, `recipes.json`,
//!   `loot_tables.json`, `textures/atlas.json`, `shaders.json`): every copy comes back
//!   base-first and the caller merges, by key (later packs replace or extend) or by appending
//!   (recipes). A pack states only what it changes, never a full copy. Keyed-row catalogs
//!   share one overlay rule, [`merge_rows`]: replace in place, `"enabled": false` removes,
//!   `"before"` places a new row.
//!
//! # Integrations
//!
//! A pack can ship `integrations/<mod id>/` dirs, laid out like the pack itself (catalogs,
//! `textures/`, `ui/documents/`). Such an overlay joins the load only while `<mod id>` names an
//! installed, enabled pack. It holds content the shipping pack states in the other pack's
//! vocabulary, and is simply absent when that pack isn't there. Its keys still carry the
//! shipping pack's id and count against the same id budget. It merges after every plain pack
//! layer, since an integration knows both packs and is the most specific statement in the
//! overlay. See [`layers`].
//!
//! # Explicit discovery
//!
//! Nothing here is discovered behind the caller's back. [`PackRoots`] names the roots
//! (`PackRoots::from_env` is the only reader of those env vars and dirs), [`PackSet::discover`]
//! admits the packs under them, and `crate::content` builds the registry from that value. The
//! free functions ([`read_layers`], [`read_bytes`], ...) are compatibility readers over the
//! current registry's pack set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

mod discover;

pub use discover::{
    admit_pack_dir, discovery_started, installed_root_active, shipped_pack_ids, PackHeader,
    PackOrigin, PackRefusal,
};

#[derive(Clone, Debug, Default)]
pub struct PackRoots {
    pub assets: Vec<PathBuf>,
    pub mods: Vec<PathBuf>,
    pub installed: Option<PathBuf>,
}

impl PackRoots {
    pub fn from_env() -> PackRoots {
        let (mods, installed) = env_mod_roots();
        PackRoots {
            assets: env_asset_roots(),
            mods,
            installed,
        }
    }

    pub fn with_mods(mods: impl IntoIterator<Item = PathBuf>) -> PackRoots {
        PackRoots {
            assets: env_asset_roots(),
            mods: mods.into_iter().collect(),
            installed: None,
        }
    }
}

fn env_asset_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("PETRAMOND_ASSETS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from("assets"));
    roots.push(workspace_root().join("assets"));
    for dir in install_dirs() {
        roots.push(dir.join("assets"));
        roots.push(dir);
    }
    roots
}

fn env_mod_roots() -> (Vec<PathBuf>, Option<PathBuf>) {
    if let Ok(dir) = std::env::var("PETRAMOND_MODS") {
        return (vec![PathBuf::from(dir)], None);
    }
    let mut shipped = Vec::new();
    let workspace = workspace_root();
    if workspace.join("Cargo.toml").is_file() {
        shipped.push(workspace.join("mods"));
    }
    for dir in install_dirs() {
        shipped.push(dir.join("mods"));
    }
    (shipped, Some(petramond_util::paths::installed_mods_dir()))
}

/// Where a shipped install keeps `assets/` and `mods/`: beside the executable, or in the app
/// bundle's `Contents/Resources` when the executable sits in `Contents/MacOS`.
fn install_dirs() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    let mut dirs = vec![dir.clone()];
    if dir.ends_with("Contents/MacOS") {
        if let Some(contents) = dir.parent() {
            dirs.push(contents.join("Resources"));
        }
    }
    dirs
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/petramond-world sits two levels below the workspace root")
        .to_path_buf()
}

#[derive(Clone)]
pub struct Pack {
    pub dir: PathBuf,
    pub header: PackHeader,
    pub origin: PackOrigin,
    pub wasm: Option<PathBuf>,
    pub client_wasm: Option<PathBuf>,
    pub integrations: Vec<Integration>,
    pub launch: Option<LaunchEntry>,
}

impl std::ops::Deref for Pack {
    type Target = PackHeader;
    fn deref(&self) -> &PackHeader {
        &self.header
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchEntry {
    pub label: String,
    pub icon: PathBuf,
}

pub const LAUNCH_LABEL_MAX: usize = 48;

#[derive(Clone)]
pub struct Integration {
    pub target: String,
    pub dir: PathBuf,
}

#[derive(Clone)]
pub struct Layer {
    pub dir: PathBuf,
    pub owner: Option<String>,
    pub requires: Vec<String>,
}

/// One discovery's packs, and the subset a world enables.
///
/// Two views, on purpose:
///
/// - **Asset layers** ([`layers`](Self::layers): point files, the tile and
///   block-model manifests, UI documents) span every INSTALLED pack. The
///   client bakes those into GPU atlases once, so they must not change when a
///   world switches mods off — and a reskin pack never needed an id anyway.
/// - **Catalog layers** ([`catalog_layers`](Self::catalog_layers): the id
///   and row catalogs — blocks, items, loot, mobs, ...) span only the ENABLED
///   packs, so a disabled mod registers no ids, no shape kinds and no rows in
///   a world that switched it off.
///
/// A fresh discovery enables everything; [`enabled`](Self::enabled) derives a
/// world's view from it.
#[derive(Clone)]
pub struct PackSet {
    base_roots: Arc<[PathBuf]>,
    installed: Arc<[Pack]>,
    refused: Arc<[PackRefusal]>,
    layers: Vec<Layer>,
    enabled: Vec<Pack>,
    catalog_layers: Vec<Layer>,
    disabled: BTreeSet<String>,
}

impl PackSet {
    pub fn discover(roots: &PackRoots) -> PackSet {
        discover::discover(roots)
    }

    fn assemble(
        base_roots: Vec<PathBuf>,
        installed: Vec<Pack>,
        refused: Vec<PackRefusal>,
    ) -> PackSet {
        let layers = layers_of(&installed);
        PackSet {
            base_roots: base_roots.into(),
            enabled: installed.clone(),
            catalog_layers: layers.clone(),
            installed: installed.into(),
            refused: refused.into(),
            layers,
            disabled: BTreeSet::new(),
        }
    }

    pub fn enabled(&self, disabled: &BTreeSet<String>) -> PackSet {
        let mut off = disabled.clone();
        loop {
            let before = off.len();
            for pack in self.installed.iter() {
                let Some(id) = &pack.id else { continue };
                if off.contains(id) {
                    continue;
                }
                if let Some(dep) = pack.dependencies.iter().find(|d| off.contains(*d)) {
                    log::info!(
                        "mod pack '{}' disabled for this world: it depends on '{dep}'",
                        pack.name
                    );
                    off.insert(id.clone());
                }
            }
            if off.len() == before {
                break;
            }
        }
        let enabled: Vec<Pack> = self
            .installed
            .iter()
            .filter(|p| p.id.as_ref().is_none_or(|id| !off.contains(id)))
            .cloned()
            .collect();
        PackSet {
            base_roots: self.base_roots.clone(),
            installed: self.installed.clone(),
            refused: self.refused.clone(),
            layers: self.layers.clone(),
            catalog_layers: layers_of(&enabled),
            enabled,
            disabled: off,
        }
    }

    pub fn installed(&self) -> &[Pack] {
        &self.installed
    }

    pub fn packs(&self) -> &[Pack] {
        &self.enabled
    }

    pub fn disabled(&self) -> &BTreeSet<String> {
        &self.disabled
    }

    pub fn refused(&self) -> &[PackRefusal] {
        &self.refused
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn catalog_layers(&self) -> &[Layer] {
        &self.catalog_layers
    }

    pub fn candidate_paths(&self, rel: &str) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.layers.iter().rev().map(|l| l.dir.join(rel)).collect();
        paths.extend(self.base_roots.iter().map(|r| r.join(rel)));
        paths
    }

    pub fn read_bytes(&self, rel: &str) -> Option<(Vec<u8>, PathBuf)> {
        self.candidate_paths(rel)
            .into_iter()
            .find_map(|path| std::fs::read(&path).ok().map(|b| (b, path)))
    }

    pub fn layer_dirs_with_ids(&self, rel: &str) -> Vec<(PathBuf, Option<String>)> {
        let mut out: Vec<(PathBuf, Option<String>)> = self
            .base_roots
            .iter()
            .rev()
            .map(|r| (r.join(rel), None))
            .collect();
        out.extend(
            self.layers
                .iter()
                .map(|l| (l.dir.join(rel), l.owner.clone())),
        );
        out.retain(|(p, _)| p.is_dir());
        out
    }

    pub fn read_catalog_layers(&self, rel: &str) -> Vec<CatalogLayer> {
        self.read_over(rel, &self.catalog_layers)
    }

    pub fn read_layers(&self, rel: &str) -> Vec<(String, PathBuf)> {
        texts_and_paths(self.read_catalog_layers(rel))
    }

    pub fn read_asset_layers(&self, rel: &str) -> Vec<(String, PathBuf)> {
        texts_and_paths(self.read_over(rel, &self.layers))
    }

    fn read_over(&self, rel: &str, layers: &[Layer]) -> Vec<CatalogLayer> {
        let mut out = Vec::new();
        for root in self.base_roots.iter() {
            let path = root.join(rel);
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push(CatalogLayer {
                    text,
                    path,
                    owner: None,
                    requires: Vec::new(),
                });
                break;
            }
        }
        for layer in layers {
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
}

fn texts_and_paths(layers: Vec<CatalogLayer>) -> Vec<(String, PathBuf)> {
    layers
        .into_iter()
        .map(|layer| (layer.text, layer.path))
        .collect()
}

fn layers_of(packs: &[Pack]) -> Vec<Layer> {
    let present: BTreeSet<&str> = packs.iter().filter_map(|p| p.id.as_deref()).collect();
    let mut out: Vec<Layer> = packs
        .iter()
        .map(|p| Layer {
            dir: p.dir.clone(),
            owner: p.id.clone(),
            requires: p.id.iter().cloned().collect(),
        })
        .collect();
    for pack in packs {
        for integration in &pack.integrations {
            if !present.contains(integration.target.as_str()) {
                continue;
            }
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
}

pub struct CatalogLayer {
    pub text: String,
    pub path: PathBuf,
    pub owner: Option<String>,
    pub requires: Vec<String>,
}

pub fn packs() -> &'static [Pack] {
    crate::content::current().packs().installed()
}

pub fn refused() -> &'static [PackRefusal] {
    crate::content::current().packs().refused()
}

pub fn layers() -> &'static [Layer] {
    crate::content::current().packs().layers()
}

pub fn candidate_paths(rel: &str) -> Vec<PathBuf> {
    crate::content::current().packs().candidate_paths(rel)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn read_base_text(rel: &str) -> Option<(String, PathBuf)> {
    env_asset_roots().into_iter().find_map(|root| {
        let path = root.join(rel);
        std::fs::read_to_string(&path).ok().map(|s| (s, path))
    })
}

pub fn read_bytes(rel: &str) -> Option<(Vec<u8>, PathBuf)> {
    crate::content::current().packs().read_bytes(rel)
}

pub fn layer_dirs_with_ids(rel: &str) -> Vec<(PathBuf, Option<String>)> {
    crate::content::current().packs().layer_dirs_with_ids(rel)
}

pub fn read_catalog_layers(rel: &str) -> Vec<CatalogLayer> {
    crate::content::current().packs().read_catalog_layers(rel)
}

pub fn read_layers(rel: &str) -> Vec<(String, PathBuf)> {
    crate::content::current().packs().read_layers(rel)
}

pub fn merge_rows(
    into: &mut Vec<Value>,
    value: Option<&Value>,
    what: &str,
    key: &str,
) -> Result<(), String> {
    for (i, row) in list(value, what)?.iter().enumerate() {
        let Some(o) = row.as_object() else {
            return Err(format!("{what}[{i}]: expected an object"));
        };
        let Some(id) = o.get(key).and_then(Value::as_str) else {
            return Err(format!("{what}[{i}]: a row names its `{key}`"));
        };
        let enabled = match o.get("enabled") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(format!("{what}[{id}].enabled: expected true or false")),
        };
        let before = match o.get("before") {
            None => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(format!("{what}[{id}].before: expected a `{key}`")),
        };
        let mut row = o.clone();
        row.remove("enabled");
        row.remove("before");
        let row = Value::Object(row);
        let named = |r: &Value, name: &str| r.get(key).and_then(Value::as_str) == Some(name);
        let existing = into.iter().position(|r| named(r, id));
        if existing.is_some() && before.is_some() {
            return Err(format!(
                "{what}[{id}].before: `before` on a row that replaces `{id}`"
            ));
        }
        match (existing, enabled) {
            (Some(at), false) => {
                into.remove(at);
            }
            (Some(at), true) => into[at] = row,
            (None, false) => {}
            (None, true) => match before {
                Some(target) => {
                    let at = into
                        .iter()
                        .position(|r| named(r, &target))
                        .ok_or_else(|| format!("{what}[{id}].before: no row named `{target}`"))?;
                    into.insert(at, row);
                }
                None => into.push(row),
            },
        }
    }
    Ok(())
}

pub fn merge_object(
    into: &mut Map<String, Value>,
    value: Option<&Value>,
    what: &str,
) -> Result<(), String> {
    match value {
        None => Ok(()),
        Some(Value::Object(o)) => {
            for (k, v) in o {
                into.insert(k.clone(), v.clone());
            }
            Ok(())
        }
        Some(_) => Err(format!("{what}: expected an object")),
    }
}

pub fn union(into: &mut Vec<Value>, value: Option<&Value>, what: &str) -> Result<(), String> {
    for name in list(value, what)? {
        if !into.contains(name) {
            into.push(name.clone());
        }
    }
    Ok(())
}

fn list<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a [Value], String> {
    match value {
        None => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(format!("{what}: expected a list")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<Value> {
        serde_json::from_str(text).unwrap()
    }

    fn ids(rows: &[Value]) -> Vec<&str> {
        rows.iter().map(|r| r["id"].as_str().unwrap()).collect()
    }

    #[test]
    fn keyed_rows_overlay_by_key_and_a_before_on_a_replacement_is_refused() {
        let mut into = rows(r#"[{"id": "a", "v": 1}, {"id": "b", "v": 1}, {"id": "c", "v": 1}]"#);
        let overlay = serde_json::from_str(
            r#"[
                {"id": "b", "v": 2},
                {"id": "c", "enabled": false},
                {"id": "d", "before": "a", "v": 1},
                {"id": "e", "v": 1},
                {"id": "zz", "enabled": false}
            ]"#,
        )
        .unwrap();
        merge_rows(&mut into, Some(&overlay), "rules", "id").unwrap();
        assert_eq!(ids(&into), ["d", "a", "b", "e"]);
        assert_eq!(into[2]["v"], 2, "a keyed row replaces in place");
        assert!(
            into[0].get("before").is_none(),
            "the directive keys are stripped"
        );

        let moved = serde_json::from_str(r#"[{"id": "a", "before": "b"}]"#).unwrap();
        let err = merge_rows(&mut into, Some(&moved), "rules", "id").unwrap_err();
        assert_eq!(err, "rules[a].before: `before` on a row that replaces `a`");
        let unkeyed = serde_json::from_str(r#"[{"v": 1}]"#).unwrap();
        assert!(merge_rows(&mut into, Some(&unkeyed), "rules", "id")
            .unwrap_err()
            .starts_with("rules[0]"));
    }
}
