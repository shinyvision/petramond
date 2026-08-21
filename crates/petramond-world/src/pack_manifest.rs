//! Pack manifest semantics: load-order resolution and namespace-prefix
//! validation. Pure functions — `crate::assets::packs()` does the filesystem
//! walking and feeds them.
//!
//! Load order = topological sort by `dependencies` + `after`, ties broken by
//! directory name (deterministic across machines — part of the mod
//! determinism contract). A pack with a missing/disabled dependency or inside
//! a dependency cycle is DISABLED, never partially loaded, and the disable
//! cascades to its dependents. `after` is ordering-only: a missing `after`
//! target is simply ignored.

use std::collections::HashMap;

/// The order-relevant slice of a pack's manifest.
pub struct PackMeta {
    /// Directory name — unique (discovery dedups), the deterministic tie-break.
    pub dir_name: String,
    /// The pack's namespace. Required when the pack ships wasm or namespaced
    /// content; content-only override packs may omit it.
    pub id: Option<String>,
    /// Whether the manifest declares a wasm module.
    pub wasm: bool,
    /// Hard requirements (ids): missing ⇒ this pack is disabled.
    pub dependencies: Vec<String>,
    /// Soft ordering (ids): load after these when present.
    pub after: Vec<String>,
}

/// A valid mod id: non-empty snake_case (`[a-z0-9_]+`), stable, not the
/// reserved engine namespace, and used as the `id:` prefix of every registry key
/// the pack introduces.
pub fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id != crate::registry::ENGINE_NAMESPACE
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The namespaced catalog keys `keys` that `pack_id` may NOT introduce: every
/// `ns:name` key must carry the pack's own id as `ns`. The reserved `petramond:*`
/// namespace belongs to base engine content, not packs. A pack without an id
/// may introduce no namespaced keys at all.
pub fn foreign_namespaced_keys(pack_id: Option<&str>, keys: &[String]) -> Vec<String> {
    keys.iter()
        .filter(|key| {
            if !crate::registry::is_namespaced(key) {
                return false;
            }
            let ns = key.split_once(':').map(|(ns, _)| ns);
            ns != pack_id
        })
        .cloned()
        .collect()
}

/// Resolve the pack load order (indices into `packs`). Disabled packs are
/// reported through `disable(index, reason)` and omitted from the result.
pub fn resolve_load_order(packs: &[PackMeta], mut disable: impl FnMut(usize, &str)) -> Vec<usize> {
    let mut alive = vec![true; packs.len()];
    let mut kill = |alive: &mut Vec<bool>, i: usize, why: &str| {
        alive[i] = false;
        disable(i, why);
    };

    // Manifest validity + unique ids (first pack in directory order wins).
    let mut ids: HashMap<&str, usize> = HashMap::new();
    for (i, p) in packs.iter().enumerate() {
        match &p.id {
            Some(id) if !valid_mod_id(id) => {
                kill(
                    &mut alive,
                    i,
                    &format!("invalid mod id '{id}' (snake_case: [a-z0-9_]+)"),
                );
            }
            Some(id) => {
                if let Some(&first) = ids.get(id.as_str()) {
                    kill(
                        &mut alive,
                        i,
                        &format!(
                            "duplicate mod id '{id}' (already provided by '{}')",
                            packs[first].dir_name
                        ),
                    );
                } else {
                    ids.insert(id, i);
                }
            }
            None if p.wasm => {
                kill(
                    &mut alive,
                    i,
                    "the pack ships wasm but its pack.json has no 'id'",
                );
            }
            None => {}
        }
    }

    // Missing-dependency cascade to a fixpoint: disabling one pack can strand
    // its dependents, transitively.
    loop {
        let mut changed = false;
        for i in 0..packs.len() {
            if !alive[i] {
                continue;
            }
            if let Some(dep) = packs[i]
                .dependencies
                .iter()
                .find(|dep| !ids.get(dep.as_str()).is_some_and(|&j| alive[j]))
            {
                kill(&mut alive, i, &format!("missing dependency '{dep}'"));
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Kahn's algorithm; the ready set is drained in directory-name order so
    // unconstrained packs keep the pre-2b ordering and ties are deterministic.
    let index_of = |id: &str| ids.get(id).copied().filter(|&j| alive[j]);
    let mut indegree = vec![0usize; packs.len()];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); packs.len()];
    for (i, p) in packs.iter().enumerate() {
        if !alive[i] {
            continue;
        }
        for dep in p.dependencies.iter().chain(&p.after) {
            if let Some(j) = index_of(dep) {
                if j != i {
                    indegree[i] += 1;
                    dependents[j].push(i);
                }
            }
        }
    }
    let mut ready: Vec<usize> = (0..packs.len())
        .filter(|&i| alive[i] && indegree[i] == 0)
        .collect();
    // Directory names are unique, so this comparison is a total order.
    ready.sort_by(|&a, &b| packs[b].dir_name.cmp(&packs[a].dir_name)); // reversed: pop() takes the smallest
    let mut order = Vec::new();
    while let Some(i) = ready.pop() {
        order.push(i);
        for &d in &dependents[i] {
            indegree[d] -= 1;
            if indegree[d] == 0 {
                let at = ready.partition_point(|&r| packs[r].dir_name > packs[d].dir_name);
                ready.insert(at, d);
            }
        }
    }
    if order.len() < alive.iter().filter(|&&a| a).count() {
        for i in 0..packs.len() {
            if alive[i] && !order.contains(&i) {
                kill(
                    &mut alive,
                    i,
                    "dependency cycle (via 'dependencies'/'after')",
                );
            }
        }
    }
    order
}

/// One admission-checked catalog file: where its registration-relevant row
/// keys live, plus any extra whole-file validation the owning loader wants run
/// at admission (so a malformed pack file disables the PACK instead of
/// panicking the shared catalog load later). New catalog quirks extend this
/// table — never a hardcoded filename branch in the loop.
struct CatalogSpec {
    rel: &'static str,
    /// Top-level array field holding the rows.
    array: &'static str,
    /// Per-row field carrying the registering key.
    key_field: &'static str,
    /// `Some((field, value))`: only rows where `field == value` contribute a
    /// key (other rows are skipped); `None`: every row must carry one.
    row_filter: Option<RowFilter>,
    /// Loader-owned extra validation over the raw file text.
    extra_validate: Option<ExtraValidate>,
}

/// A `(field, value)` pair a catalog's rows must match to be taken.
type RowFilter = (&'static str, &'static str);

/// A catalog's own check over its raw file text, beyond the shared row rules.
type ExtraValidate = fn(&str) -> Result<(), String>;

const CATALOGS: [CatalogSpec; 12] = {
    const fn plain(rel: &'static str, array: &'static str, key_field: &'static str) -> CatalogSpec {
        CatalogSpec {
            rel,
            array,
            key_field,
            row_filter: None,
            extra_validate: None,
        }
    }
    [
        plain("blocks.json", "blocks", "block"),
        plain("items.json", "items", "item"),
        plain("sounds.json", "sounds", "sound"),
        plain("models.json", "models", "key"),
        CatalogSpec {
            // `brain_extensions` register nothing but must fail admission
            // when malformed — the loader owns that check.
            extra_validate: Some(crate::ai_vocab::validate_brain_extensions),
            ..plain("mobs.json", "mobs", "mob")
        },
        plain("effects.json", "effects", "effect"),
        plain("particle_emitters.json", "emitters", "emitter"),
        plain("textures/atlas.json", "tiles", "name"),
        // EVERY recipe row — crafting and processing alike — carries a
        // namespaced `recipe` id, so both are ownership-checked here.
        plain("recipes.json", "recipes", "recipe"),
        // Custom shape declarations (WASM-baked geometry).
        plain("shapes.json", "shapes", "key"),
        plain("features.json", "features", "feature"),
        plain(
            "underground_biomes.json",
            "underground_biomes",
            "underground_biome",
        ),
    ]
};

/// The catalogs whose runtime ids are shared across every enabled pack, so
/// the number of distinct registered names has a ceiling
/// (`registry::NameTable::build`). These are the two an id budget has to be
/// enforced for; everything else is either uncapped or process-local.
pub const ID_CAPPED_CATALOGS: [&str; 2] = ["blocks.json", "items.json"];

/// The id ceiling those catalogs share — `registry::WIDE_ID_CAP`, restated
/// here so the admission rule and the registry can never drift apart (the
/// assertion below is the guard).
pub const ID_CAP: usize = crate::registry::WIDE_ID_CAP;

/// Collect every registration-relevant catalog key the pack at `dir` states —
/// the row keys of registry catalogs plus player-crafting recipe ids and atlas
/// tile names. Used for namespace-prefix validation before the pack is
/// admitted to the overlay. A malformed catalog is an error (the pack gets
/// disabled rather than panicking the registry bootstrap later).
pub fn registration_keys(dir: &std::path::Path) -> Result<Vec<String>, String> {
    Ok(registration_keys_by_catalog(dir)?
        .into_iter()
        .flat_map(|(_, keys)| keys)
        .collect())
}

/// [`registration_keys`], kept per catalog file — what the id budget needs,
/// because blocks and items own SEPARATE id tables and a pack's cost against
/// each is its own row count.
pub fn registration_keys_by_catalog(
    dir: &std::path::Path,
) -> Result<Vec<(&'static str, Vec<String>)>, String> {
    let mut out = Vec::new();
    for spec in &CATALOGS {
        let rel = spec.rel;
        let mut keys = Vec::new();
        let path = dir.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // the pack doesn't layer this catalog
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{rel}: invalid JSON: {e}"))?;
        let rows = value
            .get(spec.array)
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{rel}: expected a top-level '{}' array", spec.array))?;
        for (i, row) in rows.iter().enumerate() {
            // A `{"patch": ..., "data": ...}` row attaches data to an
            // EXISTING row — deliberately cross-namespace (describing your
            // item in a consumer mod's vocabulary), so it REGISTERS nothing
            // and is exempt from the ownership check. The loader validates
            // its shape and target.
            if row.get("patch").is_some() {
                continue;
            }
            if let Some((field, wanted)) = spec.row_filter {
                if row.get(field).and_then(|v| v.as_str()) != Some(wanted) {
                    continue;
                }
            }
            let key = row
                .get(spec.key_field)
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("{rel}: row #{i} has no string '{}' key", spec.key_field))?;
            keys.push(key.to_owned());
        }
        if let Some(validate) = spec.extra_validate {
            validate(&text).map_err(|e| format!("{rel}: {e}"))?;
        }
        out.push((rel, keys));
    }
    Ok(out)
}

/// Which packs (indices into `costs`, in the given LOAD order) must be dropped
/// to keep one shared-id catalog inside [`ID_CAP`], given the names the engine
/// already occupies.
///
/// It is a per-PACK admission rule rather than a global assertion because that
/// is the only form that degrades: the alternative — and what happened before
/// this existed — is that the shared registry bootstrap panics after the
/// catalogs merge, so installing one pack too many makes the game refuse to
/// start with no indication of which pack to remove and no way back except
/// editing the mods directory by hand.
///
/// A key already registered (engine row, or an earlier pack's) costs nothing:
/// an override is not a new id, exactly as `NameTable::build` counts it. Later
/// packs are dropped in load order, so an id budget is spent by the packs that
/// were there first and the outcome does not depend on discovery order.
pub fn id_budget_overflow(engine_names: &[&str], costs: &[Vec<String>]) -> Vec<(usize, usize)> {
    let mut taken: std::collections::HashSet<&str> = engine_names.iter().copied().collect();
    let mut over = Vec::new();
    for (i, keys) in costs.iter().enumerate() {
        let fresh: Vec<&str> = keys
            .iter()
            .map(String::as_str)
            .filter(|k| !taken.contains(k))
            .collect();
        // Count the pack's own duplicates once — the merge does.
        let distinct: std::collections::HashSet<&str> = fresh.iter().copied().collect();
        if taken.len() + distinct.len() > ID_CAP {
            over.push((i, taken.len() + distinct.len()));
            continue;
        }
        taken.extend(distinct);
    }
    over
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(dir: &str, id: Option<&str>, deps: &[&str], after: &[&str]) -> PackMeta {
        PackMeta {
            dir_name: dir.into(),
            id: id.map(str::to_owned),
            wasm: id.is_some(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            after: after.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn order_of(packs: &[PackMeta]) -> (Vec<String>, Vec<(String, String)>) {
        let mut disabled = Vec::new();
        let order = resolve_load_order(packs, |i, why| {
            disabled.push((packs[i].dir_name.clone(), why.to_owned()))
        });
        (
            order.iter().map(|&i| packs[i].dir_name.clone()).collect(),
            disabled,
        )
    }

    #[test]
    fn load_order_topo_sorts_dependencies_with_dir_name_tiebreak() {
        // c depends on a; z is unconstrained; "after" pulls b behind z.
        let packs = [
            meta("c", Some("c"), &["a"], &[]),
            meta("z", Some("z"), &[], &[]),
            meta("b", Some("b"), &[], &["z"]),
            meta("a", Some("a"), &[], &[]),
        ];
        let (order, disabled) = order_of(&packs);
        assert!(disabled.is_empty(), "{disabled:?}");
        // a < c (dependency), z < b (after); ties resolve by directory name:
        // ready sets are {a, b?, z} → a, then {c, z} → c, z, then b.
        assert_eq!(order, ["a", "c", "z", "b"]);

        // Determinism under permutation: same input set, any discovery order,
        // same result.
        let permuted = [
            meta("a", Some("a"), &[], &[]),
            meta("b", Some("b"), &[], &["z"]),
            meta("c", Some("c"), &["a"], &[]),
            meta("z", Some("z"), &[], &[]),
        ];
        let (order2, _) = order_of(&permuted);
        assert_eq!(order, order2);

        // No constraints at all = pure directory-name order (the pre-2b
        // contract packs already rely on for registry id assignment).
        let plain = [meta("20_b", None, &[], &[]), meta("10_a", None, &[], &[])];
        let (order, disabled) = order_of(&plain);
        assert!(disabled.is_empty());
        assert_eq!(order, ["10_a", "20_b"]);
    }

    #[test]
    fn missing_dependency_disables_the_mod_and_its_dependents() {
        let packs = [
            meta("lanterns", Some("lanterns"), &["glow_core"], &[]),
            meta("graves", Some("graves"), &["lanterns"], &[]),
            meta("wheel", Some("wheel"), &[], &[]),
        ];
        let (order, disabled) = order_of(&packs);
        assert_eq!(order, ["wheel"], "unaffected packs still load");
        let names: Vec<&str> = disabled.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"lanterns") && names.contains(&"graves"));
        assert!(disabled.iter().all(|(_, why)| why.contains("dependency")));

        // A dependency cycle disables every member, loudly, and spares the rest.
        let cyclic = [
            meta("a", Some("a"), &["b"], &[]),
            meta("b", Some("b"), &["a"], &[]),
            meta("c", Some("c"), &[], &[]),
        ];
        let (order, disabled) = order_of(&cyclic);
        assert_eq!(order, ["c"]);
        assert_eq!(disabled.len(), 2);
        assert!(disabled.iter().all(|(_, why)| why.contains("cycle")));
    }

    #[test]
    fn manifest_validity_rules_disable_bad_packs() {
        // wasm without id; malformed id; duplicate id (first in dir order wins).
        let mut nameless = meta("nameless", None, &[], &[]);
        nameless.wasm = true;
        let packs = [
            nameless,
            meta("badid", Some("Bad-Id"), &[], &[]),
            meta("one", Some("dupe"), &[], &[]),
            meta("two", Some("dupe"), &[], &[]),
        ];
        let (order, disabled) = order_of(&packs);
        assert_eq!(order, ["one"]);
        assert_eq!(disabled.len(), 3);
    }

    #[test]
    fn foreign_namespace_keys_flag_violations() {
        let keys = vec![
            "stone".to_owned(),           // bare non-registry string: ignored here
            "lights:lamp".to_owned(),     // own namespace
            "other:thing".to_owned(),     // someone else's
            "petramond:stone".to_owned(), // reserved engine namespace
        ];
        assert_eq!(
            foreign_namespaced_keys(Some("lights"), &keys),
            vec!["other:thing".to_owned(), "petramond:stone".to_owned()]
        );
        // Without an id, ANY namespaced key is a violation.
        assert_eq!(
            foreign_namespaced_keys(None, &keys),
            vec![
                "lights:lamp".to_owned(),
                "other:thing".to_owned(),
                "petramond:stone".to_owned()
            ]
        );
        assert!(foreign_namespaced_keys(Some("lights"), &["stone".to_owned()]).is_empty());

        assert!(valid_mod_id("day_night2"));
        for bad in ["", "Day", "day-night", "day night", "dæy", "petramond"] {
            assert!(!valid_mod_id(bad), "{bad}");
        }
    }

    #[test]
    fn crafting_recipe_ids_join_pack_namespace_validation() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "petramond-recipe-manifest-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create fixture");
        std::fs::write(
            dir.join("recipes.json"),
            r#"{"recipes":[
                {"type":"crafting","recipe":"fixture:tool"},
                {"type":"processing","recipe":"fixture:bake","class":"fixture:cooking"}
            ]}"#,
        )
        .expect("write fixture");

        let keys = registration_keys(&dir).expect("catalog parses");
        let _ = std::fs::remove_dir_all(&dir);

        // Both row kinds register: a processing row is a recipe a pack owns
        // and another pack may retire, so it is namespace-checked too.
        assert_eq!(keys, vec!["fixture:tool", "fixture:bake"]);
        assert!(foreign_namespaced_keys(Some("fixture"), &keys).is_empty());
        assert_eq!(foreign_namespaced_keys(Some("other"), &keys), keys);
    }

    /// Blocks and items share one id table, so the enabled pack set has a
    /// ceiling. The invariant worth pinning is not the number — it is that
    /// hitting it costs the OFFENDING pack and nothing else: the packs ahead
    /// of it in load order keep their ids (so saves keep resolving), the ones
    /// behind it are still considered, and an override is not a new id.
    #[test]
    fn the_shared_id_ceiling_disables_the_pack_that_crosses_it() {
        let engine: Vec<String> = (0..ID_CAP - 6).map(|i| format!("petramond:e{i}")).collect();
        let engine: Vec<&str> = engine.iter().map(String::as_str).collect();
        let pack = |n: usize, prefix: &str| -> Vec<String> {
            (0..n).map(|i| format!("{prefix}:r{i}")).collect()
        };

        // `ID_CAP - 6` plus 4 fits; the next pack's 5 would cross, so IT is
        // dropped and the pack after it — which fits in what is left — still
        // loads.
        let costs = [pack(4, "a"), pack(5, "b"), pack(2, "c")];
        assert_eq!(
            id_budget_overflow(&engine, &costs)
                .iter()
                .map(|(i, _)| *i)
                .collect::<Vec<_>>(),
            vec![1]
        );

        // An OVERRIDE is not a new id: a pack restating engine rows is free,
        // and so is restating a row an earlier pack registered.
        let overrides: Vec<String> = engine.iter().take(20).map(|s| (*s).to_owned()).collect();
        let costs = [pack(5, "a"), overrides, pack(1, "a")];
        assert!(id_budget_overflow(&engine, &costs).is_empty());

        // Nothing installed at all cannot overflow, and neither can a pack
        // whose rows are all duplicates of each other.
        assert!(id_budget_overflow(&engine, &[]).is_empty());
        let dupes = vec!["d:one".to_owned(); 40];
        assert!(id_budget_overflow(&engine, &[dupes]).is_empty());
    }

    /// `brain_extensions` register no keys, but a malformed block must fail
    /// ADMISSION (pack disabled) — never reach the catalog load, whose
    /// extension pre-pass would panic the registry bootstrap.
    #[test]
    fn malformed_brain_extensions_fail_pack_admission() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "petramond-brainext-manifest-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create fixture");

        let write = |json: &str| std::fs::write(dir.join("mobs.json"), json).expect("write");
        write(
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep","brain":[{"node":"fixture:lure","priority":20,"inputs":["player_held"]}]}]}"#,
        );
        let keys = registration_keys(&dir).expect("a well-formed extension passes admission");
        assert!(keys.is_empty(), "extensions register no keys");

        // Missing `brain` field, an unknown field, an unknown node key, an
        // unknown declared input, and inputs on an engine node — all must be
        // admission errors (pack disabled), not later catalog-load panics.
        for bad in [
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep"}]}"#,
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep","brains":[]}]}"#,
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep","brain":[{"node":"chasse_player"}]}]}"#,
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep","brain":[{"node":"fixture:lure","inputs":["player_hand"]}]}]}"#,
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep","brain":[{"node":"wander","inputs":["player_held"]}]}]}"#,
        ] {
            write(bad);
            let err = registration_keys(&dir).expect_err("malformed extension fails admission");
            assert!(err.contains("brain_extensions"), "{err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
