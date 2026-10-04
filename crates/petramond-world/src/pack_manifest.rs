use std::collections::HashMap;

pub struct PackMeta {
    pub dir_name: String,
    pub id: Option<String>,
    pub wasm: bool,
    pub dependencies: Vec<String>,
    pub after: Vec<String>,
}

/// How much of one resource a mod expects to need, from its `pack.json` `resources` section.
/// A tier only sets where the mod starts: the engine's watchdog lets any mod grow past it as long
/// as the growth is gradual.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    #[default]
    Standard,
    Heavy,
    Extreme,
}

/// A mod's declared needs, per kind of work and per resource. Every field is optional in
/// `pack.json`; an unknown field or tier refuses the pack, so a typo never silently falls back.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceNeeds {
    /// `mod_init`: one-off work at load.
    pub init: Tier,
    /// Tick systems, event handlers and block hooks on the server tick.
    pub tick: Tier,
    /// Mob AI decisions.
    pub ai: Tier,
    /// Client frames, UI and canvas callbacks.
    pub client: Tier,
    /// Worldgen features and stage replacements.
    pub worldgen: Tier,
    pub memory: Tier,
    /// World KV the mod keeps in the save.
    pub storage: Tier,
}

pub fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id != crate::registry::ENGINE_NAMESPACE
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

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

pub fn resolve_load_order(packs: &[PackMeta], mut disable: impl FnMut(usize, &str)) -> Vec<usize> {
    let mut alive = vec![true; packs.len()];
    let mut kill = |alive: &mut Vec<bool>, i: usize, why: &str| {
        alive[i] = false;
        disable(i, why);
    };

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
    ready.sort_by(|&a, &b| packs[b].dir_name.cmp(&packs[a].dir_name));
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

struct CatalogSpec {
    rel: &'static str,
    array: &'static str,
    key_field: &'static str,
    row_filter: Option<RowFilter>,
    extra_validate: Option<ExtraValidate>,
    affects_world: bool,
}

type RowFilter = (&'static str, &'static str);

type ExtraValidate = fn(&str) -> Result<(), String>;

const CATALOGS: [CatalogSpec; 19] = {
    const fn world(rel: &'static str, array: &'static str, key_field: &'static str) -> CatalogSpec {
        CatalogSpec {
            rel,
            array,
            key_field,
            row_filter: None,
            extra_validate: None,
            affects_world: true,
        }
    }
    const fn presentation(
        rel: &'static str,
        array: &'static str,
        key_field: &'static str,
    ) -> CatalogSpec {
        CatalogSpec {
            affects_world: false,
            ..world(rel, array, key_field)
        }
    }
    [
        world("blocks.json", "blocks", "block"),
        world("items.json", "items", "item"),
        presentation("sounds.json", "sounds", "sound"),
        presentation("music.json", "tracks", "track"),
        presentation("models.json", "models", "key"),
        presentation("animated_models.json", "animated_models", "model"),
        CatalogSpec {
            extra_validate: Some(crate::ai_vocab::validate_brain_extensions),
            ..world("mobs.json", "mobs", "mob")
        },
        world("effects.json", "effects", "effect"),
        world("conditions.json", "conditions", "condition"),
        presentation("particle_emitters.json", "emitters", "emitter"),
        presentation("cloth.json", "cloths", "cloth"),
        presentation("textures/atlas.json", "tiles", "name"),
        world("recipes.json", "recipes", "recipe"),
        world("shapes.json", "shapes", "key"),
        world("features.json", "features", "feature"),
        world("excavations.json", "excavations", "excavation"),
        world("structures.json", "structures", "structure"),
        world("loot_tables.json", "loot_tables", "loot"),
        world(
            "underground_biomes.json",
            "underground_biomes",
            "underground_biome",
        ),
    ]
};

pub const ID_CAPPED_CATALOGS: [&str; 2] = ["blocks.json", "items.json"];

pub const ID_CAP: usize = crate::registry::WIDE_ID_CAP;

pub fn registration_keys(dir: &std::path::Path) -> Result<Vec<String>, String> {
    Ok(registration_keys_by_catalog(dir)?
        .into_iter()
        .flat_map(|(_, keys)| keys)
        .collect())
}

pub fn registration_keys_by_catalog(
    dir: &std::path::Path,
) -> Result<Vec<(&'static str, Vec<String>)>, String> {
    let mut out = Vec::new();
    for spec in &CATALOGS {
        let rel = spec.rel;
        let mut keys = Vec::new();
        let path = dir.join(rel);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{rel}: invalid JSON: {e}"))?;
        let rows = value
            .get(spec.array)
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("{rel}: expected a top-level '{}' array", spec.array))?;
        for (i, row) in rows.iter().enumerate() {
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

pub fn states_world_rows(dir: &std::path::Path) -> Result<bool, String> {
    for spec in CATALOGS.iter().filter(|spec| spec.affects_world) {
        let Ok(text) = std::fs::read_to_string(dir.join(spec.rel)) else {
            continue;
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: invalid JSON: {e}", spec.rel))?;
        if value
            .get(spec.array)
            .and_then(|v| v.as_array())
            .is_some_and(|rows| !rows.is_empty())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn id_budget_overflow(engine_names: &[&str], costs: &[Vec<String>]) -> Vec<(usize, usize)> {
    let mut taken: std::collections::HashSet<&str> = engine_names.iter().copied().collect();
    let mut over = Vec::new();
    for (i, keys) in costs.iter().enumerate() {
        let fresh: Vec<&str> = keys
            .iter()
            .map(String::as_str)
            .filter(|k| !taken.contains(k))
            .collect();
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
        let packs = [
            meta("c", Some("c"), &["a"], &[]),
            meta("z", Some("z"), &[], &[]),
            meta("b", Some("b"), &[], &["z"]),
            meta("a", Some("a"), &[], &[]),
        ];
        let (order, disabled) = order_of(&packs);
        assert!(disabled.is_empty(), "{disabled:?}");
        assert_eq!(order, ["a", "c", "z", "b"]);

        let permuted = [
            meta("a", Some("a"), &[], &[]),
            meta("b", Some("b"), &[], &["z"]),
            meta("c", Some("c"), &["a"], &[]),
            meta("z", Some("z"), &[], &[]),
        ];
        let (order2, _) = order_of(&permuted);
        assert_eq!(order, order2);

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
            "stone".to_owned(),
            "lights:lamp".to_owned(),
            "other:thing".to_owned(),
            "petramond:stone".to_owned(),
        ];
        assert_eq!(
            foreign_namespaced_keys(Some("lights"), &keys),
            vec!["other:thing".to_owned(), "petramond:stone".to_owned()]
        );
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
        let dir = petramond_util::test_dirs::TestScratchDir::new("recipe-manifest");
        std::fs::write(
            dir.join("recipes.json"),
            r#"{"recipes":[
                {"type":"crafting","recipe":"fixture:tool"},
                {"type":"processing","recipe":"fixture:bake","class":"fixture:cooking"}
            ]}"#,
        )
        .expect("write fixture");

        let keys = registration_keys(&dir).expect("catalog parses");

        assert_eq!(keys, vec!["fixture:tool", "fixture:bake"]);
        assert!(foreign_namespaced_keys(Some("fixture"), &keys).is_empty());
        assert_eq!(foreign_namespaced_keys(Some("other"), &keys), keys);
    }

    #[test]
    fn the_shared_id_ceiling_disables_the_pack_that_crosses_it() {
        let engine: Vec<String> = (0..ID_CAP - 6).map(|i| format!("petramond:e{i}")).collect();
        let engine: Vec<&str> = engine.iter().map(String::as_str).collect();
        let pack = |n: usize, prefix: &str| -> Vec<String> {
            (0..n).map(|i| format!("{prefix}:r{i}")).collect()
        };

        let costs = [pack(4, "a"), pack(5, "b"), pack(2, "c")];
        assert_eq!(
            id_budget_overflow(&engine, &costs)
                .iter()
                .map(|(i, _)| *i)
                .collect::<Vec<_>>(),
            vec![1]
        );

        let overrides: Vec<String> = engine.iter().take(20).map(|s| (*s).to_owned()).collect();
        let costs = [pack(5, "a"), overrides, pack(1, "a")];
        assert!(id_budget_overflow(&engine, &costs).is_empty());

        assert!(id_budget_overflow(&engine, &[]).is_empty());
        let dupes = vec!["d:one".to_owned(); 40];
        assert!(id_budget_overflow(&engine, &[dupes]).is_empty());
    }

    #[test]
    fn malformed_brain_extensions_fail_pack_admission() {
        let dir = petramond_util::test_dirs::TestScratchDir::new("brainext-manifest");

        let write = |json: &str| std::fs::write(dir.join("mobs.json"), json).expect("write");
        write(
            r#"{"mobs":[],"brain_extensions":[{"mob":"petramond:sheep","brain":[{"node":"fixture:lure","priority":20,"inputs":["player_held"]}]}]}"#,
        );
        let keys = registration_keys(&dir).expect("a well-formed extension passes admission");
        assert!(keys.is_empty(), "extensions register no keys");

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
    }
}
