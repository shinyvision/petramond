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
//! A pack is a visible directory under a `mods/` root containing a
//! `pack.json` manifest. Roots ([`PackRoots`]): the `PETRAMOND_MODS`
//! override REPLACES every other root and counts as shipped; otherwise the
//! workspace `mods/` (only in a checkout) and `mods/` beside the executable
//! are SHIPPED, and `<OS data dir>/petramond/mods` is the one INSTALLED root.
//! A shipped pack wins every directory-name or id collision with an installed
//! one; among installed packs the directory-order rule settles duplicates.
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
//!   "launch": { "label": "My Tool", "icon": "launch.png" },
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
//!   it changes, never a full copy of the catalogue. Catalogs of KEYED ROWS
//!   share one overlay rule, [`merge_rows`]: replace in place, `"enabled":
//!   false` removes, `"before"` places a new row.
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
//!
//! # Explicit discovery
//!
//! Nothing here is discovered behind the caller's back: [`PackRoots`] names
//! the roots (`PackRoots::from_env` is the only reader of the variables and
//! directories above), [`PackSet::discover`] admits the packs under them, and
//! the content registry (`crate::content`) is built from that value. The free
//! functions ([`read_layers`], [`read_bytes`], ...) are compatibility readers
//! over the CURRENT registry's pack set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};

mod discover;

pub use discover::{
    admit_pack_dir, discovery_started, installed_root_active, shipped_pack_ids, PackHeader,
    PackOrigin, PackRefusal,
};

/// Where content is searched for: base asset roots and `mods/` roots, each in
/// priority order (first wins). [`PackRoots::from_env`] is the ONE place the
/// process environment and working directory enter content loading; every
/// loader downstream reads through the [`PackSet`] discovered from a
/// `PackRoots` value, so a test (or a second world) can point at its own
/// roots without touching the environment.
#[derive(Clone, Debug, Default)]
pub struct PackRoots {
    /// Base asset directories, highest priority first.
    pub assets: Vec<PathBuf>,
    /// SHIPPED `mods/` directories searched for packs, highest priority
    /// first. Their packs are content packs: never written by the game.
    pub mods: Vec<PathBuf>,
    /// The one INSTALLED `mods/` root, when this discovery reads one. Its
    /// packs may never shadow a shipped pack's directory name or id.
    pub installed: Option<PathBuf>,
}

impl PackRoots {
    /// The launch environment's roots (see the module docs): the
    /// `PETRAMOND_ASSETS` / `PETRAMOND_MODS` overrides, the working
    /// directory's assets, the workspace checkout, the executable's
    /// directory and the user's installed-packs dir.
    pub fn from_env() -> PackRoots {
        let (mods, installed) = env_mod_roots();
        PackRoots {
            assets: env_asset_roots(),
            mods,
            installed,
        }
    }

    /// The environment's base asset roots with EXACTLY `mods` as the (shipped)
    /// pack roots — a fixture pack set staged by a test, or an explicit launch.
    pub fn with_mods(mods: impl IntoIterator<Item = PathBuf>) -> PackRoots {
        PackRoots {
            assets: env_asset_roots(),
            mods: mods.into_iter().collect(),
            installed: None,
        }
    }
}

/// Base asset directories (no packs), in priority order (first wins).
fn env_asset_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(dir) = std::env::var("PETRAMOND_ASSETS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from("assets"));
    // The workspace checkout's assets, resolved at COMPILE time — so dev/test
    // binaries of every workspace crate find them no matter which package dir
    // cargo runs them from. Shipped builds keep resolving exe-relative below.
    roots.push(workspace_root().join("assets"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.join("assets"));
            roots.push(dir.to_path_buf());
        }
    }
    roots
}

/// The shipped `mods/` roots in priority order, and the installed root (see
/// the module docs). Unlike the additive base roots, the `PETRAMOND_MODS`
/// override REPLACES the default roots and counts as shipped: pointing the
/// game at a mods dir must mean exactly that mod set, not "that plus whatever
/// the working directory carries" — so under it the installed root is not
/// read. The working directory's `mods/` is never a root.
fn env_mod_roots() -> (Vec<PathBuf>, Option<PathBuf>) {
    if let Ok(dir) = std::env::var("PETRAMOND_MODS") {
        return (vec![PathBuf::from(dir)], None);
    }
    let mut shipped = Vec::new();
    // The workspace checkout's mods, compile-time-resolved like the asset
    // roots' entry (dev/test binaries of sibling workspace crates) — only
    // while that checkout still exists.
    let workspace = workspace_root();
    if workspace.join("Cargo.toml").is_file() {
        shipped.push(workspace.join("mods"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            shipped.push(dir.join("mods"));
        }
    }
    (shipped, Some(petramond_util::paths::installed_mods_dir()))
}

/// The workspace root, from this crate's compiled-in manifest dir. Dev builds
/// run from anywhere inside the repo; installed builds never hit these roots
/// (the data-dir roots above resolve first).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/petramond-world sits two levels below the workspace root")
        .to_path_buf()
}

/// A discovered, validated mod pack in load order. Its display and
/// dependency data are its [`PackHeader`] (read through `Deref`).
#[derive(Clone)]
pub struct Pack {
    pub dir: PathBuf,
    pub header: PackHeader,
    /// The root it was found in.
    pub origin: PackOrigin,
    /// Absolute path of the pack's compiled logic, when it ships one.
    pub wasm: Option<PathBuf>,
    /// Optional presentation-only client module. It runs in a separate
    /// restricted instance and cannot mutate the deterministic simulation.
    pub client_wasm: Option<PathBuf>,
    /// The pack's `integrations/<mod id>/` overlays whose target pack is
    /// installed, by target name. Ones naming an absent pack are dropped at
    /// discovery with a logged note.
    pub integrations: Vec<Integration>,
    /// The pack's entry on the title screen, when it declares one that
    /// discovery admits.
    pub launch: Option<LaunchEntry>,
}

impl std::ops::Deref for Pack {
    type Target = PackHeader;
    fn deref(&self) -> &PackHeader {
        &self.header
    }
}

/// A title-screen launch entry (`pack.json` `launch`): an icon button that
/// starts the pack's `client_wasm` with no world behind it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchEntry {
    /// What the button's tooltip says.
    pub label: String,
    /// Absolute path of the button's icon PNG.
    pub icon: PathBuf,
}

/// The longest launch label, in bytes: the tooltip ellipsizes anything that
/// outgrows its panel, but a label this long is a pack writing a paragraph.
pub const LAUNCH_LABEL_MAX: usize = 48;

/// One `integrations/<target>/` directory of a pack (see the module docs).
#[derive(Clone)]
pub struct Integration {
    /// The mod id the overlay is written against.
    pub target: String,
    pub dir: PathBuf,
}

/// One content overlay directory in merge order: a pack, or one pack's
/// integration with another.
#[derive(Clone)]
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
    /// Discover and admit every pack under `roots.mods` (see the module docs
    /// for the admission and load-order rules). Refused packs are logged and
    /// listed in [`refused`](Self::refused); discovery itself never fails.
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

    /// This discovery scoped to a world that switched `disabled` off: those
    /// packs, every pack depending on one of them (transitively), and every
    /// integration targeting one of them leave the CATALOG layers. Asset
    /// layers keep every installed pack (see the type docs).
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

    /// Every admitted pack, in load order — whether or not this view enables it.
    pub fn installed(&self) -> &[Pack] {
        &self.installed
    }

    /// The packs this view enables, in load order.
    pub fn packs(&self) -> &[Pack] {
        &self.enabled
    }

    /// The mod ids this view switched off, dependents included.
    pub fn disabled(&self) -> &BTreeSet<String> {
        &self.disabled
    }

    /// The packs discovery refused, with the reason — what a load report shows
    /// beside the content errors.
    pub fn refused(&self) -> &[PackRefusal] {
        &self.refused
    }

    /// Every installed overlay in merge order: packs in load order, then their
    /// integrations in the same order. Base `assets/` is not a layer here; the
    /// readers below put it first.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// [`layers`](Self::layers) restricted to the enabled packs.
    pub fn catalog_layers(&self) -> &[Layer] {
        &self.catalog_layers
    }

    /// Candidate absolute paths for the asset at `rel` (e.g. `recipes.json`),
    /// in priority order: packs (highest priority first), then the base roots.
    pub fn candidate_paths(&self, rel: &str) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self.layers.iter().rev().map(|l| l.dir.join(rel)).collect();
        paths.extend(self.base_roots.iter().map(|r| r.join(rel)));
        paths
    }

    /// Read the first readable candidate for `rel` as raw bytes (textures,
    /// models, sounds), or `None` if no candidate exists.
    pub fn read_bytes(&self, rel: &str) -> Option<(Vec<u8>, PathBuf)> {
        self.candidate_paths(rel)
            .into_iter()
            .find_map(|path| std::fs::read(&path).ok().map(|b| (b, path)))
    }

    /// Existing directories for `rel` across the base roots + packs, LOWEST
    /// priority first — callers overlay their contents by filename, later dirs
    /// winning (e.g. a pack's baked GUI shadows the base one of the same
    /// name). Each directory carries its owning pack namespace id (`None` for
    /// base dirs and id-less override packs) so loaders can validate
    /// namespaced content against the pack that ships it (mod GUI kinds).
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

    /// Every copy of the ID/ROW catalog `rel` over the ENABLED packs, with
    /// each layer's overlay identity, lowest priority first. Recipe loading
    /// needs the identity so disabling a pack removes even rows that
    /// reference engine content only, and an integration's rows go with
    /// EITHER of its packs.
    pub fn read_catalog_layers(&self, rel: &str) -> Vec<CatalogLayer> {
        self.read_over(rel, &self.catalog_layers)
    }

    /// Read EVERY copy of the layered catalog `rel` over the enabled packs,
    /// lowest priority first: the base file (from the first base root that
    /// has it), then each pack's copy in load order. The caller merges layers
    /// by its catalogue's key semantics. Empty if nothing provides the file.
    pub fn read_layers(&self, rel: &str) -> Vec<(String, PathBuf)> {
        texts_and_paths(self.read_catalog_layers(rel))
    }

    /// [`read_layers`](Self::read_layers) over EVERY installed pack — for the
    /// presentation manifests the client bakes once (the tile atlas, block
    /// models; see the type docs).
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
                break; // base roots shadow each other; only one base layer
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

/// The overlays of `packs` in merge order: the packs in load order, then their
/// integrations in the same order — an integration only while its target is
/// among `packs`.
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

// ---------------------------------------------------------------------------
// Compatibility accessors over the thread's CURRENT content registry
// (`crate::content::current`), for call sites that still read assets
// ambiently. Loaders building a registry read its `PackSet` directly.
// ---------------------------------------------------------------------------

/// Every installed pack of the current registry, in load order.
pub fn packs() -> &'static [Pack] {
    crate::content::current().packs().installed()
}

/// The packs the current registry's discovery refused, with why.
pub fn refused() -> &'static [PackRefusal] {
    crate::content::current().packs().refused()
}

/// The current registry's asset overlays ([`PackSet::layers`]).
pub fn layers() -> &'static [Layer] {
    crate::content::current().packs().layers()
}

/// [`PackSet::candidate_paths`] of the current registry.
pub fn candidate_paths(rel: &str) -> Vec<PathBuf> {
    crate::content::current().packs().candidate_paths(rel)
}

/// Read the shipped BASE copy of `rel` from the environment's asset roots —
/// packs deliberately excluded — with the path it loaded from, or `None` if
/// no base root has it. This is the loaders' shipped-file test gate: "the
/// base catalog is valid on its own" must not change meaning because a mod
/// pack happens to be installed, and it needs no registry at all.
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_base_text(rel: &str) -> Option<(String, PathBuf)> {
    env_asset_roots().into_iter().find_map(|root| {
        let path = root.join(rel);
        std::fs::read_to_string(&path).ok().map(|s| (s, path))
    })
}

/// [`PackSet::read_bytes`] of the current registry.
pub fn read_bytes(rel: &str) -> Option<(Vec<u8>, PathBuf)> {
    crate::content::current().packs().read_bytes(rel)
}

/// [`PackSet::layer_dirs_with_ids`] of the current registry.
pub fn layer_dirs_with_ids(rel: &str) -> Vec<(PathBuf, Option<String>)> {
    crate::content::current().packs().layer_dirs_with_ids(rel)
}

/// [`PackSet::read_catalog_layers`] of the current registry.
pub fn read_catalog_layers(rel: &str) -> Vec<CatalogLayer> {
    crate::content::current().packs().read_catalog_layers(rel)
}

/// [`PackSet::read_layers`] of the current registry.
pub fn read_layers(rel: &str) -> Vec<(String, PathBuf)> {
    crate::content::current().packs().read_layers(rel)
}

/// The keyed-row catalog overlay rule: merge `value`'s rows into `into` by
/// their `key` — a row keyed like an existing one replaces it in place,
/// `"enabled": false` removes it, a new row appends or goes `"before"` the
/// row it names. Errors are qualified by `what` (the field being merged).
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

/// Merge `value`'s entries into `into` by key, later entries replacing.
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

/// Append every entry of the list `value` that `into` lacks.
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

    /// The keyed-row overlay: replace in place, remove, append, insert
    /// before — and a `before` on a row that replaces one is refused rather
    /// than silently ignored, since the author asked for a move the rule
    /// cannot honour.
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
