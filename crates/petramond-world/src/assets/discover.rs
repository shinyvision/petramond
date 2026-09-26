//! Pack discovery and admission: walk the `mods/` roots, validate each pack's
//! manifest and files, resolve the load order, and enforce the id budget.
//! Produces the [`PackSet`] every content loader reads through.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{Integration, Pack, PackRoots, PackSet};
use crate::pack_manifest::{self as manifest, PackMeta};

/// A pack discovery refused, and why. The pack is logged and left out whole —
/// packs never load partially.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackRefusal {
    /// The pack's directory name (its identity before a manifest is trusted).
    pub dir_name: String,
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

/// Collects refusals while logging each one as it happens.
#[derive(Default)]
struct Refusals(Vec<PackRefusal>);

impl Refusals {
    fn refuse(&mut self, dir_name: &str, reason: String) {
        log::error!("mod pack '{dir_name}' disabled: {reason}");
        self.0.push(PackRefusal {
            dir_name: dir_name.to_owned(),
            reason,
        });
    }
}

/// The `integrations/<name>/` subdirectories under a pack dir, by name.
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

/// The directories whose catalogs a pack contributes given the installed id
/// set: its own, plus each integration whose target is installed.
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

pub(super) fn discover(roots: &PackRoots) -> PackSet {
    let mut refusals = Refusals::default();

    // Gather candidates: the FIRST root providing a pack directory name wins
    // (mirrors the base-root priority), so a dev-tree pack shadows an
    // installed one. Sorted by directory name = the deterministic input order.
    let mut found: Vec<(String, PathBuf, PackManifest)> = Vec::new();
    for root in &roots.mods {
        let Ok(entries) = std::fs::read_dir(root) else {
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
                Err(e) => refusals.refuse(&dir_name, format!("bad pack.json: {e}")),
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));

    // Per-pack validation that needs the pack's files: the wasm file must
    // exist, and every namespaced catalog key must carry the pack's own id.
    // A violating pack is disabled whole — never a partial load.
    found.retain(|(dir_name, dir, m)| match admission_problem(dir, m) {
        Some(why) => {
            refusals.refuse(dir_name, why);
            false
        }
        None => true,
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
        refusals.refuse(&metas[i].dir_name, why.to_owned());
    });
    let order = enforce_id_budget(&found, &metas, order, &mut refusals);
    let installed: BTreeSet<String> = order
        .iter()
        .filter_map(|&i| found[i].2.id.clone())
        .collect();

    let mut packs = Vec::with_capacity(order.len());
    let mut dependencies = Vec::with_capacity(order.len());
    for i in order {
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
        packs.push(Pack {
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
        });
        dependencies.push(m.dependencies.clone());
    }
    PackSet::assemble(roots.assets.clone(), packs, dependencies, refusals.0)
}

/// Why the pack in `dir` cannot be admitted, or `None`: declared wasm must
/// exist, and every namespaced catalog key (its integrations' included) must
/// carry the pack's own id.
fn admission_problem(dir: &Path, m: &PackManifest) -> Option<String> {
    if let Some(wasm) = &m.wasm {
        if !dir.join(wasm).is_file() {
            return Some(format!("declared wasm '{wasm}' not found in the pack"));
        }
    }
    if let Some(wasm) = &m.client_wasm {
        if !dir.join(wasm).is_file() {
            return Some(format!(
                "declared client_wasm '{wasm}' not found in the pack"
            ));
        }
    }
    let mut keys = match manifest::registration_keys(dir) {
        Ok(keys) => keys,
        Err(e) => return Some(e),
    };
    // An integration's rows are the pack's own statements, so they obey the
    // pack's namespace whether or not the target is installed.
    for (target, sub) in integration_dirs(dir) {
        match manifest::registration_keys(&sub) {
            Ok(more) => keys.extend(more),
            Err(e) => return Some(format!("integrations/{target}: {e}")),
        }
    }
    let foreign = manifest::foreign_namespaced_keys(m.id.as_deref(), &keys);
    if foreign.is_empty() {
        return None;
    }
    Some(format!(
        "namespaced catalog keys must use the pack's own id ('{}:'): {}",
        m.id.as_deref().unwrap_or("<no id>"),
        foreign.join(", ")
    ))
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
    found: &[(String, PathBuf, PackManifest)],
    metas: &[PackMeta],
    order: Vec<usize>,
    refusals: &mut Refusals,
) -> Vec<usize> {
    use crate::pack_manifest::{ID_CAP, ID_CAPPED_CATALOGS};

    // One catalog read per pack, reused for both capped catalogs. Admission
    // already parsed these files; a second read here keeps the budget rule
    // where the rest of the load-order policy lives. An integration's rows
    // cost the SHIPPING pack, and are costed against the packs found rather
    // than the final order — a target dropped below simply leaves the
    // estimate slightly generous.
    let installed: BTreeSet<String> = found.iter().filter_map(|(_, _, m)| m.id.clone()).collect();
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
            refusals.refuse(
                &metas[order[slot]].dir_name,
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
    // Re-resolve so the dependency cascade takes the dropped packs' dependents
    // with them, instead of leaving a pack running against content that is no
    // longer there.
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
        refusals.refuse(&sub[j].dir_name, why.to_owned());
    })
    .into_iter()
    .map(|j| survivors[j])
    .collect()
}
