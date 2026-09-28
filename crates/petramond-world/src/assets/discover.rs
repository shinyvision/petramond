use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Integration, LaunchEntry, Pack, PackRoots, PackSet, LAUNCH_LABEL_MAX};
use crate::pack_manifest::{self as manifest, PackMeta};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PackOrigin {
    Shipped,
    Installed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackHeader {
    pub name: String,
    pub id: Option<String>,
    pub version: Option<String>,
    pub description: String,
    pub summary: Option<String>,
    pub icon: Option<PathBuf>,
    pub dependencies: Vec<String>,
    pub touches_world: bool,
    pub resources: manifest::ResourceNeeds,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackRefusal {
    pub dir_name: String,
    pub dir: PathBuf,
    pub header: Option<PackHeader>,
    pub origin: PackOrigin,
    pub reason: String,
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
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    wasm: Option<String>,
    #[serde(default)]
    client_wasm: Option<String>,
    #[serde(default)]
    launch: Option<LaunchManifest>,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    after: Vec<String>,
    #[serde(default)]
    resources: manifest::ResourceNeeds,
}

#[derive(serde::Deserialize)]
struct LaunchManifest {
    label: String,
    icon: String,
}

struct Candidate {
    dir_name: String,
    dir: PathBuf,
    origin: PackOrigin,
    manifest: PackManifest,
    header: PackHeader,
}

#[derive(Default)]
struct Refusals(Vec<PackRefusal>);

impl Refusals {
    fn refuse(
        &mut self,
        dir_name: &str,
        dir: &Path,
        header: Option<PackHeader>,
        origin: PackOrigin,
        reason: String,
    ) {
        log::error!("mod pack '{dir_name}' disabled: {reason}");
        if self.0.iter().any(|r| r.dir == dir) {
            return;
        }
        self.0.push(PackRefusal {
            dir_name: dir_name.to_owned(),
            dir: dir.to_path_buf(),
            header,
            origin,
            reason,
        });
    }

    fn refuse_candidate(&mut self, c: &Candidate, reason: String) {
        self.refuse(
            &c.dir_name,
            &c.dir,
            Some(c.header.clone()),
            c.origin,
            reason,
        );
    }
}

fn origin_roots(roots: &PackRoots) -> Vec<(PathBuf, PackOrigin)> {
    let shipped = roots
        .mods
        .iter()
        .map(|root| (root.clone(), PackOrigin::Shipped));
    let installed = roots
        .installed
        .iter()
        .map(|root| (root.clone(), PackOrigin::Installed));
    shipped.chain(installed).collect()
}

pub fn installed_root_active() -> bool {
    PackRoots::from_env().installed.is_some()
}

pub fn shipped_pack_ids() -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for root in PackRoots::from_env().mods {
        for (_, dir) in pack_dirs(&root) {
            let manifest = std::fs::read_to_string(dir.join("pack.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<PackManifest>(&text).ok());
            if let Some(id) = manifest.and_then(|m| m.id) {
                ids.insert(id);
            }
        }
    }
    ids
}

static DISCOVERY_STARTED: AtomicBool = AtomicBool::new(false);

pub fn discovery_started() -> bool {
    DISCOVERY_STARTED.load(Ordering::Acquire)
}

fn pack_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.path();
            (!name.starts_with('.') && dir.is_dir() && dir.join("pack.json").is_file())
                .then_some((name, dir))
        })
        .collect();
    out.sort();
    out
}

fn integration_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
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

fn catalog_dirs(dir: &Path, installed: &BTreeSet<String>) -> Vec<PathBuf> {
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(
        integration_dirs(dir)
            .into_iter()
            .filter(|(target, _)| installed.contains(target))
            .map(|(_, sub)| sub),
    );
    dirs
}

fn header_of(dir: &Path, m: &PackManifest, touches_world: bool) -> PackHeader {
    PackHeader {
        name: m.name.clone(),
        id: m.id.clone(),
        version: m.version.clone(),
        description: m.description.clone(),
        summary: m.summary.clone(),
        icon: m.icon.as_ref().map(|i| dir.join(i)).filter(|p| p.is_file()),
        dependencies: m.dependencies.clone(),
        touches_world,
        resources: m.resources,
    }
}

pub fn admit_pack_dir(dir: &Path) -> Result<PackHeader, String> {
    admit(dir).map(|(header, _)| header).map_err(|(_, why)| why)
}

type Refusal = (Option<Box<PackHeader>>, String);

fn admit(dir: &Path) -> Result<(PackHeader, PackManifest), Refusal> {
    let text = std::fs::read_to_string(dir.join("pack.json"))
        .map_err(|e| (None, format!("pack.json unreadable: {e}")))?;
    let m = serde_json::from_str::<PackManifest>(&text)
        .map_err(|e| (None, format!("bad pack.json: {e}")))?;
    let refuse = |why: String| {
        let header = header_of(dir, &m, false);
        Err((Some(Box::new(header)), why))
    };
    if let Some(wasm) = &m.wasm {
        if !dir.join(wasm).is_file() {
            return refuse(format!("declared wasm '{wasm}' not found in the pack"));
        }
    }
    if let Some(wasm) = &m.client_wasm {
        if !dir.join(wasm).is_file() {
            return refuse(format!(
                "declared client_wasm '{wasm}' not found in the pack"
            ));
        }
    }
    let mut keys = match manifest::registration_keys(dir) {
        Ok(keys) => keys,
        Err(e) => return refuse(e),
    };
    let mut touches_world = m.wasm.is_some();
    match manifest::states_world_rows(dir) {
        Ok(world) => touches_world |= world,
        Err(e) => return refuse(e),
    }
    for (target, sub) in integration_dirs(dir) {
        match manifest::registration_keys(&sub) {
            Ok(more) => keys.extend(more),
            Err(e) => return refuse(format!("integrations/{target}: {e}")),
        }
        match manifest::states_world_rows(&sub) {
            Ok(world) => touches_world |= world,
            Err(e) => return refuse(format!("integrations/{target}: {e}")),
        }
    }
    let foreign = manifest::foreign_namespaced_keys(m.id.as_deref(), &keys);
    if !foreign.is_empty() {
        return refuse(format!(
            "namespaced catalog keys must use the pack's own id ('{}:'): {}",
            m.id.as_deref().unwrap_or("<no id>"),
            foreign.join(", ")
        ));
    }
    Ok((header_of(dir, &m, touches_world), m))
}

pub(super) fn discover(roots: &PackRoots) -> PackSet {
    if roots.installed.is_some() {
        DISCOVERY_STARTED.store(true, Ordering::Release);
    }
    let (packs, refused) = discover_from(origin_roots(roots));
    PackSet::assemble(roots.assets.clone(), packs, refused)
}

fn discover_from(roots: Vec<(PathBuf, PackOrigin)>) -> (Vec<Pack>, Vec<PackRefusal>) {
    let mut refusals = Refusals::default();
    let mut found: Vec<Candidate> = Vec::new();
    let mut shipped_names = BTreeSet::new();
    let mut shipped_ids = BTreeSet::new();
    for (root, origin) in roots {
        for (dir_name, dir) in pack_dirs(&root) {
            if origin == PackOrigin::Shipped && found.iter().any(|c| c.dir_name == dir_name) {
                continue;
            }
            let (header, m) = match admit(&dir) {
                Ok(admitted) => admitted,
                Err((header, reason)) => {
                    refusals.refuse(&dir_name, &dir, header.map(|h| *h), origin, reason);
                    continue;
                }
            };
            if origin == PackOrigin::Installed {
                let clash = if shipped_names.contains(&dir_name) {
                    Some(dir_name.clone())
                } else {
                    m.id.clone().filter(|id| shipped_ids.contains(id))
                };
                if let Some(id) = clash {
                    let reason = format!("uses the id of the content pack '{id}'");
                    refusals.refuse(&dir_name, &dir, Some(header), origin, reason);
                    continue;
                }
            } else {
                shipped_names.insert(dir_name.clone());
                shipped_ids.extend(m.id.clone());
            }
            found.push(Candidate {
                dir_name,
                dir,
                origin,
                manifest: m,
                header,
            });
        }
    }
    found.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));

    let metas: Vec<PackMeta> = found
        .iter()
        .map(|c| PackMeta {
            dir_name: c.dir_name.clone(),
            id: c.manifest.id.clone(),
            wasm: c.manifest.wasm.is_some() || c.manifest.client_wasm.is_some(),
            dependencies: c.manifest.dependencies.clone(),
            after: c.manifest.after.clone(),
        })
        .collect();
    let order = manifest::resolve_load_order(&metas, |i, why| {
        refusals.refuse_candidate(&found[i], why.to_owned());
    });
    let order = enforce_id_budget(&found, &metas, order, &mut refusals);
    let installed: BTreeSet<String> = order
        .iter()
        .filter_map(|&i| found[i].manifest.id.clone())
        .collect();

    let mut slots: Vec<Option<Candidate>> = found.into_iter().map(Some).collect();
    let packs = order
        .into_iter()
        .filter_map(|i| slots[i].take())
        .map(|c| {
            let m = &c.manifest;
            let dir = &c.dir;
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
                wasm: m.wasm.as_ref().map(|w| dir.join(w)),
                client_wasm: m.client_wasm.as_ref().map(|w| dir.join(w)),
                integrations,
                launch: launch_entry(&m.name, dir, m),
                origin: c.origin,
                header: c.header.clone(),
            }
        })
        .collect();
    (packs, refusals.0)
}

fn launch_entry(name: &str, dir: &Path, m: &PackManifest) -> Option<LaunchEntry> {
    let launch = m.launch.as_ref()?;
    let refuse = |why: &str| {
        log::error!("mod pack '{name}': launch entry ignored: {why}");
        None
    };
    if m.id.is_none() || m.client_wasm.is_none() {
        return refuse("only a pack with an id and a client_wasm can be launched");
    }
    let label = launch.label.trim();
    if label.is_empty() || label.len() > LAUNCH_LABEL_MAX || label.contains(['\n', '\r']) {
        return refuse(&format!(
            "the label must be one line of 1..={LAUNCH_LABEL_MAX} bytes"
        ));
    }
    let relative = Path::new(&launch.icon);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return refuse(&format!("icon '{}' must lie inside the pack", launch.icon));
    }
    let icon = dir.join(relative);
    if !icon.is_file() {
        return refuse(&format!("icon '{}' not found in the pack", launch.icon));
    }
    Some(LaunchEntry {
        label: label.to_owned(),
        icon,
    })
}

/// Drop, from the resolved load order, any pack whose block/item rows would
/// push a shared registry past its id ceiling — and re-run order resolution so
/// a dropped pack's dependents go with it.
///
/// The ceiling is real (ids are save- and wire-relevant, so the width is a
/// format decision, not a local one) but it is no longer tight: `Block` and
/// `ItemType` are `u16`. What this rule guarantees is that reaching it is an
/// ADMISSION outcome — the offending pack is disabled, loudly, and everything
/// before it still runs — rather than a failure inside the registry build
/// long after admission, which would name no pack.
fn enforce_id_budget(
    found: &[Candidate],
    metas: &[PackMeta],
    order: Vec<usize>,
    refusals: &mut Refusals,
) -> Vec<usize> {
    use crate::pack_manifest::{ID_CAP, ID_CAPPED_CATALOGS};

    let installed: BTreeSet<String> = found.iter().filter_map(|c| c.manifest.id.clone()).collect();
    let per_pack: Vec<Vec<(&'static str, Vec<String>)>> = order
        .iter()
        .map(|&i| {
            let mut merged: Vec<(&'static str, Vec<String>)> = Vec::new();
            for dir in catalog_dirs(&found[i].dir, &installed) {
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

    let mut dropped: BTreeSet<usize> = BTreeSet::new();
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
            refusals.refuse_candidate(
                &found[order[slot]],
                format!(
                    "its {rel} rows would register {would_be} names, but the registry caps at \
                     {ID_CAP}"
                ),
            );
            dropped.insert(order[slot]);
        }
    }
    if dropped.is_empty() {
        return order;
    }
    let survivors: Vec<usize> = order
        .iter()
        .copied()
        .filter(|i| !dropped.contains(i))
        .collect();
    let sub: Vec<PackMeta> = survivors
        .iter()
        .map(|&i| PackMeta {
            dir_name: metas[i].dir_name.clone(),
            id: metas[i].id.clone(),
            wasm: metas[i].wasm,
            dependencies: metas[i].dependencies.clone(),
            after: metas[i].after.clone(),
        })
        .collect();
    manifest::resolve_load_order(&sub, |j, why| {
        refusals.refuse_candidate(&found[survivors[j]], why.to_owned());
    })
    .into_iter()
    .map(|j| survivors[j])
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> petramond_util::test_dirs::TestScratchDir {
        petramond_util::test_dirs::TestScratchDir::new(&format!("discovery-{tag}"))
    }

    fn pack(root: &Path, dir: &str, manifest: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = root.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pack.json"), manifest).unwrap();
        for (rel, text) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    #[test]
    fn declared_resource_needs_parse_and_a_bad_tier_refuses_the_pack() {
        use crate::pack_manifest::{ResourceNeeds, Tier};

        let root = scratch("resources");
        let mods = root.join("mods");
        pack(
            &mods,
            "castles",
            r#"{"name": "Castles", "id": "castles", "resources": {"tick": "heavy", "memory": "extreme"}}"#,
            &[],
        );
        pack(&mods, "plain", r#"{"name": "Plain", "id": "plain"}"#, &[]);
        pack(
            &mods,
            "typo",
            r#"{"name": "Typo", "id": "typo", "resources": {"tick": "huge"}}"#,
            &[],
        );
        let (packs, refused) = discover_from(vec![(mods, PackOrigin::Shipped)]);
        let needs = |id: &str| {
            packs
                .iter()
                .find(|p| p.id.as_deref() == Some(id))
                .map(|p| p.resources)
        };
        assert_eq!(
            needs("castles"),
            Some(ResourceNeeds {
                tick: Tier::Heavy,
                memory: Tier::Extreme,
                ..Default::default()
            })
        );
        assert_eq!(needs("plain"), Some(ResourceNeeds::default()));
        assert_eq!(needs("typo"), None);
        assert!(refused.iter().any(|r| r.dir_name == "typo"), "{refused:?}");
    }

    #[test]
    fn installed_packs_never_shadow_shipped_ones_and_hidden_dirs_are_not_packs() {
        let root = scratch("shadow");
        let (shipped, installed) = (root.join("shipped"), root.join("installed"));
        pack(
            &shipped,
            "forge",
            r#"{"name": "Forge", "id": "forge"}"#,
            &[],
        );
        pack(
            &installed,
            "aaa_forge",
            r#"{"name": "Fake Forge", "id": "forge"}"#,
            &[],
        );
        pack(
            &installed,
            ".staging",
            r#"{"name": "Half", "id": "half"}"#,
            &[],
        );
        pack(
            &installed,
            "extra",
            r#"{"name": "Extra", "id": "extra"}"#,
            &[],
        );
        let (packs, refused) = discover_from(vec![
            (shipped, PackOrigin::Shipped),
            (installed, PackOrigin::Installed),
        ]);
        let loaded: Vec<_> = packs.iter().map(|p| (p.name.as_str(), p.origin)).collect();
        assert_eq!(
            loaded,
            [
                ("Extra", PackOrigin::Installed),
                ("Forge", PackOrigin::Shipped)
            ]
        );
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(
            refused[0].header.as_ref().map(|h| h.name.as_str()),
            Some("Fake Forge")
        );
        assert!(refused[0].reason.contains("content pack 'forge'"));
    }

    #[test]
    fn touching_the_world_is_derived_from_the_packs_files() {
        let root = scratch("touches");
        let admit = |dir: &str, manifest: &str, files: &[(&str, &str)]| {
            admit_pack_dir(&pack(&root, dir, manifest, files))
                .unwrap()
                .touches_world
        };
        assert!(!admit(
            "look",
            r#"{"name": "Look", "id": "look", "client_wasm": "c.wasm"}"#,
            &[
                ("c.wasm", ""),
                ("sounds.json", r#"{"sounds": []}"#),
                ("textures/atlas.json", r#"{"tiles": [{"name": "look:t"}]}"#),
            ],
        ));
        assert!(admit(
            "patcher",
            r#"{"name": "Patcher", "id": "patcher"}"#,
            &[(
                "items.json",
                r#"{"items": [{"patch": "petramond:stick", "data": {}}]}"#
            )],
        ));
        assert!(admit(
            "overlay",
            r#"{"name": "Overlay", "id": "overlay"}"#,
            &[(
                "integrations/forge/blocks.json",
                r#"{"blocks": [{"block": "overlay:mould"}]}"#
            )],
        ));
        assert!(admit(
            "logic",
            r#"{"name": "Logic", "id": "logic", "wasm": "m.wasm"}"#,
            &[("m.wasm", "")],
        ));
    }
}
