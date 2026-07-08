//! Runtime name↔id registries for pack-extensible content.
//!
//! Blocks and items are opaque `u8` ids behind newtypes (`Block(u8)`,
//! `ItemType(u8)`). Engine content owns the low ids in a compiled, frozen
//! order (worldgen parity and existing saves depend on those ids never
//! moving); engine content is named under the reserved `petramond:*` namespace.
//! Mod packs ADD content by introducing rows with their own NAMESPACED keys
//! (`mod_id:name`) in the existing layered catalogs (`blocks.json`,
//! `items.json`), which register fresh ids after the engine range in pack
//! load order. Bare names are not registry keys.
//!
//! This module owns the NAME side of that contract: the id-ordered name
//! tables both serde (`Block`/`ItemType` (de)serialize as their name string)
//! and the save palette identify content by. The full definition tables are
//! still owned by their loaders (`block::load`, `item::load`); they resolve
//! rows against these same tables so ids can never disagree.
//!
//! Blocks and items get one SHARED bootstrap (`names()`) because their
//! catalogs cross-reference (block drops name items; a dynamic item's `block`
//! field names a block) — resolving through one table pair avoids any lazy-init
//! cycle between the two loaders.

use std::sync::LazyLock;

use serde::Deserialize;

/// Reserved namespace for engine-owned public keys.
pub(crate) const ENGINE_NAMESPACE: &str = "petramond";

/// An id-ordered list of registered names: the compiled engine names first
/// (index == frozen engine id), then pack-registered namespaced names in load
/// order. Ids are `u8` — content tables stay 256 entries max.
#[derive(Debug)]
pub(crate) struct NameTable {
    names: Vec<&'static str>,
}

impl NameTable {
    /// The runtime id of `name`, or `None` if it is not registered.
    pub fn id(&self, name: &str) -> Option<u8> {
        self.names.iter().position(|&n| n == name).map(|i| i as u8)
    }

    /// The registered name for `id`, or `None` if out of range.
    pub fn name(&self, id: u8) -> Option<&'static str> {
        self.names.get(id as usize).copied()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Build a table from the compiled engine names plus every layer's row
    /// keys in order. A key that is an engine name (or an already-registered
    /// dynamic name) is an override — no new id. A non-`petramond` NAMESPACED key
    /// (`mod_id:name`) registers the next id. Bare keys and unknown `petramond:*`
    /// keys are errors.
    pub fn build(
        engine: &[&'static str],
        layer_keys: &[Vec<String>],
        what: &str,
    ) -> Result<NameTable, String> {
        let mut names: Vec<&'static str> = engine.to_vec();
        for keys in layer_keys {
            for key in keys {
                if names.iter().any(|n| n == key) {
                    continue; // engine override or dynamic re-statement
                }
                if !is_namespaced(key) {
                    return Err(format!(
                        "unknown {what} '{key}': registry keys must be namespaced; use a known \
                         engine key like 'petramond:name' or a mod-owned 'mod_id:name' key"
                    ));
                }
                if namespace(key) == Some(ENGINE_NAMESPACE) {
                    return Err(format!(
                        "unknown {what} '{key}': the '{ENGINE_NAMESPACE}' namespace is reserved \
                         for engine-owned keys"
                    ));
                }
                names.push(Box::leak(key.clone().into_boxed_str()));
            }
        }
        if names.len() > 256 {
            return Err(format!(
                "{} {what}s registered, but ids are one byte: the registry caps at 256 \
                 (engine uses {}; remove or merge pack content)",
                names.len(),
                engine.len()
            ));
        }
        Ok(NameTable { names })
    }
}

/// Whether `key` carries a `namespace:` prefix.
pub(crate) fn is_namespaced(key: &str) -> bool {
    namespace(key).is_some()
}

/// The namespace of `key` (`"wheel:wheel" → Some("wheel")`,
/// `"petramond:stone" → Some("petramond")`), or `None` for bare and degenerate forms.
/// The per-world mod enablement gates (palette / recipes / natural spawner)
/// key off this.
pub(crate) fn namespace(key: &str) -> Option<&str> {
    match key.split_once(':') {
        Some((ns, name)) if !ns.is_empty() && !name.is_empty() => Some(ns),
        _ => None,
    }
}

/// Extensible tag vocabulary: compiled engine tags own the low ids (bare
/// snake_case names, also reachable as `petramond:<name>`); packs add NAMESPACED
/// tags (`mod_id:name`), interned on first sight during load — a tag is
/// *defined by being listed* (on a data row or in a recipe), it has no
/// standalone declaration. Ids are process-local and never persisted, so
/// intern order only needs to be self-consistent within a run; runtime tag
/// checks compare ids, no lock taken.
pub(crate) struct TagTable {
    engine: &'static [&'static str],
    dynamic: std::sync::RwLock<Vec<&'static str>>,
}

impl TagTable {
    pub(crate) const fn new(engine: &'static [&'static str]) -> Self {
        Self {
            engine,
            dynamic: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// Resolve a tag name from data: a bare name must be an engine tag (typo
    /// guard — a misspelled engine tag must not silently become a new tag);
    /// `petramond:<engine>` resolves to the same id; a namespaced `mod_id:name`
    /// interns on first sight.
    pub(crate) fn resolve(&self, name: &str) -> Result<u8, String> {
        let bare = name.strip_prefix("petramond:").unwrap_or(name);
        if let Some(i) = self.engine.iter().position(|n| *n == bare) {
            return Ok(i as u8);
        }
        if !is_namespaced(name) {
            return Err(format!(
                "unknown tag '{name}' (engine tags: {}; mod tags must be namespaced 'mod_id:name')",
                self.engine.join(", ")
            ));
        }
        let mut dynamic = self.dynamic.write().unwrap();
        if let Some(i) = dynamic.iter().position(|n| *n == name) {
            return Ok((self.engine.len() + i) as u8);
        }
        let id = self.engine.len() + dynamic.len();
        if id > u8::MAX as usize {
            return Err(format!("tag table full registering '{name}' (256 max)"));
        }
        dynamic.push(Box::leak(name.to_owned().into_boxed_str()));
        Ok(id as u8)
    }

    /// The registered name for `id` (diagnostics only).
    #[allow(dead_code)]
    pub(crate) fn name(&self, id: u8) -> &'static str {
        let id = id as usize;
        if id < self.engine.len() {
            return self.engine[id];
        }
        self.dynamic
            .read()
            .unwrap()
            .get(id - self.engine.len())
            .copied()
            .unwrap_or("?")
    }
}

/// The block + item name tables (see module docs).
pub(crate) struct ContentNames {
    pub blocks: NameTable,
    pub items: NameTable,
}

/// Build both tables from raw catalog layer texts — the pure core `names()`
/// wraps, split out so loader tests can drive it with synthetic layers. Only
/// the row KEYS are read here; full row validation stays with the loaders.
pub(crate) fn build_names(
    block_texts: &[&str],
    item_texts: &[&str],
) -> Result<ContentNames, String> {
    #[derive(Deserialize)]
    struct BlockKeys {
        blocks: Vec<BlockKey>,
    }
    #[derive(Deserialize)]
    struct BlockKey {
        block: String,
    }
    #[derive(Deserialize)]
    struct ItemKeys {
        items: Vec<ItemKey>,
    }
    #[derive(Deserialize)]
    struct ItemKey {
        item: String,
    }

    let mut block_keys = Vec::new();
    for (li, text) in block_texts.iter().enumerate() {
        let raw: BlockKeys = serde_json::from_str(text)
            .map_err(|e| format!("blocks.json layer #{li}: invalid JSON: {e}"))?;
        block_keys.push(raw.blocks.into_iter().map(|r| r.block).collect());
    }
    let mut item_keys = Vec::new();
    for (li, text) in item_texts.iter().enumerate() {
        let raw: ItemKeys = serde_json::from_str(text)
            .map_err(|e| format!("items.json layer #{li}: invalid JSON: {e}"))?;
        item_keys.push(raw.items.into_iter().map(|r| r.item).collect());
    }
    Ok(ContentNames {
        blocks: NameTable::build(crate::block::ENGINE_BLOCK_NAMES, &block_keys, "block")?,
        items: NameTable::build(crate::item::ENGINE_ITEM_NAMES, &item_keys, "item")?,
    })
}

/// The process-wide name tables, built once from the real catalog layers
/// (base `assets/` + packs). Loads on first touch from any thread; a bad pack
/// key fails loudly here, before any definition table builds on top of it.
pub(crate) fn names() -> &'static ContentNames {
    static NAMES: LazyLock<ContentNames> = LazyLock::new(|| {
        let blocks = crate::assets::read_layers("blocks.json");
        let items = crate::assets::read_layers("items.json");
        let block_texts: Vec<&str> = blocks.iter().map(|(s, _)| s.as_str()).collect();
        let item_texts: Vec<&str> = items.iter().map(|(s, _)| s.as_str()).collect();
        build_names(&block_texts, &item_texts).unwrap_or_else(|e| panic!("content registry: {e}"))
    });
    &NAMES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_table_interns_namespaced_and_rejects_bare_unknowns() {
        let t = TagTable::new(&["fuel", "planks"]);
        assert_eq!(t.resolve("fuel"), Ok(0));
        assert_eq!(
            t.resolve("petramond:planks"),
            Ok(1),
            "an engine tag resolves under its namespaced recipe form too"
        );
        let a = t.resolve("mymod:ores").expect("namespaced tags intern");
        assert_eq!(t.resolve("mymod:ores"), Ok(a), "stable on re-resolution");
        assert_eq!(t.name(a), "mymod:ores");
        assert!(
            t.resolve("orees").is_err(),
            "a bare unknown is a typo'd engine tag, never a silent new tag"
        );
    }

    #[test]
    fn namespaced_keys_register_and_bare_unknowns_error() {
        let engine = &["petramond:air", "petramond:stone"];
        // Engine override (known `petramond:*`) + a namespaced addition.
        let table = NameTable::build(
            engine,
            &[vec!["petramond:stone".into(), "mymod:gadget".into()]],
            "block",
        )
        .expect("valid layers");
        assert_eq!(table.len(), 3, "override adds no id; the addition does");
        assert_eq!(
            table.id("petramond:stone"),
            Some(1),
            "engine ids never move"
        );
        assert_eq!(table.id("mymod:gadget"), Some(2), "appended after engine");
        assert_eq!(table.name(2), Some("mymod:gadget"));
        // Restating a registered dynamic name in a later layer adds no id.
        let table = NameTable::build(
            engine,
            &[vec!["mymod:gadget".into()], vec!["mymod:gadget".into()]],
            "block",
        )
        .unwrap();
        assert_eq!(table.len(), 3);
        // A NEW bare name is an error, not a registration.
        let err = NameTable::build(engine, &[vec!["gadget".into()]], "block")
            .expect_err("bare additions are refused");
        assert!(err.contains("gadget") && err.contains("namespace"), "{err}");
        let err = NameTable::build(engine, &[vec!["petramond:gadget".into()]], "block")
            .expect_err("unknown engine-namespace additions are refused");
        assert!(
            err.contains("petramond") && err.contains("reserved"),
            "{err}"
        );
        // Degenerate namespaces are not namespaces.
        for bad in [":gadget", "mymod:", ":"] {
            assert!(!is_namespaced(bad), "{bad}");
        }
        assert!(is_namespaced("mymod:gadget"));
    }

    #[test]
    fn registry_caps_at_256_ids() {
        let engine = &["petramond:air"];
        let keys: Vec<String> = (0..256).map(|i| format!("mymod:thing_{i}")).collect();
        let err = NameTable::build(engine, &[keys], "block").expect_err("cap enforced");
        assert!(err.contains("256"), "{err}");
    }

    /// End-to-end dynamic registration: a real pack (blocks.json + items.json
    /// under a `PETRAMOND_MODS` dir) registers a namespaced block + item, the
    /// block is placeable/breakable through `World`, and the save palette pins
    /// the dynamic entry by name with engine ids stable.
    ///
    /// The global registries are process-wide LazyLocks, so pack injection
    /// must happen before ANY test touches them — this outer test spawns the
    /// test binary again as a child with the env set, running only the
    /// `#[ignore]`d inner test below. Deterministic regardless of test order.
    #[test]
    fn dynamic_pack_content_flows_end_to_end() {
        let root = std::env::temp_dir().join(format!("petramond-dynpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("mods/testmod");
        std::fs::create_dir_all(&pack).unwrap();
        // `id` is mandatory since 2b: the pack introduces `testmod:` keys, and
        // namespaced keys must carry the owning pack's id.
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Test Mod", "id": "testmod", "description": "dynamic registration fixture" }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("blocks.json"),
            r#"{ "blocks": [ { "block": "testmod:glowrock", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 28, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [{"item": "testmod:glowrock", "min": 1, "max": 1, "chance": 1.0}] } ] }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("items.json"),
            r#"{ "items": [ { "item": "testmod:glowrock", "key": "testmod:glowrock", "name": "Glowrock", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "testmod:glowrock" } ] }"#,
        )
        .unwrap();

        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(exe)
            .arg("registry::tests::dynamic_pack_world_inner")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("PETRAMOND_MODS", root.join("mods"))
            .env("PETRAMOND_DYNPACK_SAVE", root.join("save"))
            .output()
            .expect("spawn test binary");
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            out.status.success(),
            "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
    /// pointing at the fixture pack before first registry touch).
    #[test]
    #[ignore = "spawned by dynamic_pack_content_flows_end_to_end with a fixture pack env"]
    fn dynamic_pack_world_inner() {
        use crate::block::Block;
        use crate::chunk::{Chunk, ChunkPos};
        use crate::item::ItemType;
        use crate::world::World;

        let engine_blocks = crate::block::ENGINE_BLOCK_NAMES.len();
        let engine_items = crate::item::ENGINE_ITEM_NAMES.len();

        // --- Registration: one fresh id past each engine set, name-addressed. ---
        assert_eq!(Block::all().len(), engine_blocks + 1);
        assert_eq!(ItemType::all().len(), engine_items + 1);
        let glow = Block(engine_blocks as u8);
        let glow_item = ItemType(engine_items as u8);
        assert_eq!(names().blocks.id("testmod:glowrock"), Some(glow.0));
        // Serde speaks registry names for dynamic content too.
        assert_eq!(
            serde_json::to_value(glow).unwrap(),
            serde_json::Value::String("testmod:glowrock".into())
        );
        assert_eq!(
            serde_json::from_value::<Block>(serde_json::Value::String("testmod:glowrock".into()))
                .unwrap(),
            glow
        );

        // --- The def resolved like any engine row's. ---
        assert!(glow.is_solid() && glow.is_opaque());
        assert_eq!(glow.behavior().key(), "inert");
        assert_eq!(glow.light_emission(), 28);
        assert_eq!(glow.hardness(), 2.0);
        assert_eq!(glow.drop_spec().drops.len(), 1);
        assert_eq!(glow.drop_spec().drops[0].item, glow_item);
        // The item links back to its block both ways.
        assert_eq!(glow_item.as_block(), Some(glow));
        assert_eq!(ItemType::from_block(glow), glow_item);
        assert_eq!(glow.to_item(), glow_item);

        // --- Placeable + breakable through World. ---
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let (x, y, z) = (5, 64, 5);
        assert!(w.set_block_world(x, y, z, glow), "placement succeeds");
        assert_eq!(Block::from_id(w.chunk_block(x, y, z)), glow);
        assert!(
            !w.collision_boxes_at(x, y, z).is_empty(),
            "the placed block collides via its row's boxes"
        );
        assert!(w.set_block_world(x, y, z, Block::Air), "break succeeds");
        assert_eq!(Block::from_id(w.chunk_block(x, y, z)), Block::Air);

        // --- Save palette: dynamic entry pinned by name, engine ids stable. ---
        let save = std::path::PathBuf::from(std::env::var_os("PETRAMOND_DYNPACK_SAVE").unwrap());
        // An "old" palette written before the mod existed, with a stranger
        // entry so disk ids and runtime ids genuinely diverge.
        std::fs::create_dir_all(&save).unwrap();
        let mut blocks: Vec<&str> = crate::block::ENGINE_BLOCK_NAMES.to_vec();
        blocks.push("othermod:stranger");
        let items: Vec<&str> = crate::item::ENGINE_ITEM_NAMES.to_vec();
        std::fs::write(
            save.join("palette.json"),
            serde_json::json!({ "blocks": blocks, "items": items }).to_string(),
        )
        .unwrap();
        let p = crate::save::palette::load_or_create(&save, &Default::default()).unwrap();
        for &b in Block::all() {
            assert_eq!(p.block_from_disk(p.block_to_disk(b.id())), b.id(), "{b:?}");
        }
        for id in 0..engine_blocks as u8 {
            assert_eq!(
                p.block_to_disk(id),
                id,
                "engine block ids are identity here"
            );
        }
        // The dynamic block was appended AFTER the stranger, so its disk id
        // differs from its runtime id — the palette remaps by name.
        assert_eq!(p.block_to_disk(glow.0), engine_blocks as u8 + 1);
        let text = std::fs::read_to_string(save.join("palette.json")).unwrap();
        assert!(
            text.contains("testmod:glowrock"),
            "the dynamic entry is pinned in palette.json"
        );
    }
}
