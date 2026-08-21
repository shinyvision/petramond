//! Runtime name↔id registries for pack-extensible content.
//!
//! Blocks and items are opaque `u16` ids behind newtypes (`Block(u16)`,
//! `ItemType(u16)`); the smaller catalogs (mobs, sounds, effects, emitters,
//! models, features, biomes) stay one byte. Engine content owns the low ids in
//! a compiled, frozen
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

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// Reserved namespace for engine-owned public keys.
pub const ENGINE_NAMESPACE: &str = "petramond";

/// Id ceiling for the catalogs whose ids ride the save record and the wire as
/// TWO bytes — blocks and items. Sixteen times the old one-byte ceiling, which
/// is the whole point: the enabled pack set no longer shares 256 names per
/// catalog. Dense per-id tables are sized to the registry's actual length, not
/// to this number, so raising it costs nothing until the ids are used.
pub const WIDE_ID_CAP: usize = 4096;

/// Free ids below which [`names`] warns at boot: an ordinary content pack
/// registers a few dozen rows, so this is "one more pack might not fit".
pub const ID_HEADROOM_WARN: usize = 128;

/// Id ceiling for the catalogs whose ids are still ONE byte (mobs, sounds,
/// effects, emitters, models, features, biomes). None of them is near it — the
/// shipped set uses at most 50 of 256 — and each has a `u8` wire field or
/// dense table behind it, so this is the honest cap for them.
pub const BYTE_ID_CAP: usize = 256;

/// An id-ordered list of registered names: the compiled engine names first
/// (index == frozen engine id), then pack-registered namespaced names in load
/// order. Ids are `u16`; each catalog declares its own ceiling
/// ([`WIDE_ID_CAP`] / [`BYTE_ID_CAP`]) when it builds the table. Carries a
/// name→id hash index built once here, so every name lookup (serde, palette,
/// net remap, host calls) is O(1).
#[derive(Debug)]
pub struct NameTable {
    names: Vec<&'static str>,
    ids: HashMap<&'static str, u16>,
}

impl NameTable {
    /// The runtime id of `name`, or `None` if it is not registered.
    pub fn id(&self, name: &str) -> Option<u16> {
        self.ids.get(name).copied()
    }

    /// The registered name for `id`, or `None` if out of range.
    pub fn name(&self, id: u16) -> Option<&'static str> {
        self.names.get(id as usize).copied()
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn push(&mut self, name: &'static str) {
        self.ids.insert(name, self.names.len() as u16);
        self.names.push(name);
    }

    /// Build a table from the compiled engine names plus every layer's row
    /// keys in order. A key that is an engine name (or an already-registered
    /// dynamic name) is an override — no new id. A non-`petramond` NAMESPACED key
    /// (`mod_id:name`) registers the next id. Bare keys and unknown `petramond:*`
    /// keys are errors. `cap` is the catalog's id ceiling — the same number
    /// `modding::manifest` costs packs against at admission.
    pub fn build(
        engine: &[&'static str],
        layer_keys: &[Vec<String>],
        what: &str,
        cap: usize,
    ) -> Result<NameTable, String> {
        let mut table = NameTable {
            names: Vec::with_capacity(engine.len()),
            ids: HashMap::with_capacity(engine.len()),
        };
        for &name in engine {
            table.push(name);
        }
        for keys in layer_keys {
            for key in keys {
                if table.ids.contains_key(key.as_str()) {
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
                table.push(Box::leak(key.clone().into_boxed_str()));
            }
        }
        if table.names.len() > cap {
            return Err(format!(
                "{} {what}s registered, but the registry caps at {cap} \
                 (engine uses {}; remove or merge pack content)",
                table.names.len(),
                engine.len()
            ));
        }
        Ok(table)
    }
}

/// A loaded content registry: the id-ordered definition rows plus the
/// [`NameTable`] that assigned their ids. The table is DENSE (every registered
/// name covered exactly once), so `id(name)` always indexes a valid row — the
/// one uniform name→id lookup for every catalog consumer; an unknown name is
/// `None`, and what that degrades to (air, MISSING, skip) stays the caller's
/// policy.
pub struct Catalog<D: 'static> {
    rows: &'static [D],
    names: NameTable,
}

impl<D> Catalog<D> {
    /// The id-ordered definition rows (`rows()[id]` is `id`'s row).
    pub fn rows(&self) -> &'static [D] {
        self.rows
    }

    /// The runtime id registered under `name` (engine `petramond:*` and pack
    /// `mod_id:name` keys alike), or `None` when no such row is loaded.
    pub fn id(&self, name: &str) -> Option<u16> {
        self.names.id(name)
    }
}

/// The shared layered-catalog load frame the content registries
/// (`effects.json`, `sounds.json`, `models.json`, `blocks.json`, ...) speak:
/// parse each layer's row list, merge rows by registry key (a later layer's
/// row REPLACES the earlier one, so a pack states only the rows it changes or
/// adds), build the name table from the compiled engine names plus the
/// layers' own keys (engine names hold their frozen ids, namespaced keys
/// register after them in load order — see [`NameTable::build`]), then
/// `convert` every merged row and demand a dense table: every registered
/// name covered exactly once, ids contiguous with no holes.
///
/// `convert` gets the row, its resolved id, and the name table (for the
/// interned `&'static` name and cross-row references).
pub fn load_catalog<R, D>(
    texts: &[&str],
    parse_layer: impl FnMut(&str) -> Result<Vec<R>, serde_json::Error>,
    row_key: fn(&R) -> &str,
    engine: &[&'static str],
    what: &str,
    convert: impl FnMut(R, u16, &NameTable) -> Result<D, String>,
) -> Result<Catalog<D>, String> {
    let (merged, layer_keys) = parse_and_merge(texts, parse_layer, row_key)?;
    let names = NameTable::build(engine, &layer_keys, what, BYTE_ID_CAP)?;
    let rows = resolve_merged(merged, row_key, &names, what, convert)?;
    Ok(Catalog {
        rows: Box::leak(rows.into_boxed_slice()),
        names,
    })
}

/// [`load_catalog`] against a PREBUILT name table — for the catalogs whose
/// names bootstrap elsewhere (blocks and items share [`names`], so their id
/// assignment already happened there).
pub fn resolve_catalog<R, D>(
    texts: &[&str],
    parse_layer: impl FnMut(&str) -> Result<Vec<R>, serde_json::Error>,
    row_key: fn(&R) -> &str,
    names: &NameTable,
    what: &str,
    convert: impl FnMut(R, u16, &NameTable) -> Result<D, String>,
) -> Result<Vec<D>, String> {
    let (merged, _) = parse_and_merge(texts, parse_layer, row_key)?;
    resolve_merged(merged, row_key, names, what, convert)
}

/// One `{"patch": "<row>", "data": {...}}` row from a catalog layer: attaches
/// namespaced DATA entries to an EXISTING row — engine rows included, and
/// deliberately across namespaces (a pack describing its target in a
/// CONSUMER mod's vocabulary is the whole point; see [`compile_data_map`]).
/// A patch can, by construction, touch ONLY the row's `data` map — never
/// behavior, shape, or any other field.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDataPatch {
    /// Registry name of the row being patched (must exist after all layers).
    pub patch: String,
    pub data: serde_json::Map<String, serde_json::Value>,
}

/// Split one catalog layer's row array (`{"<array_key>": [...]}`) into full
/// rows and data patches: an element carrying a `"patch"` field parses as
/// [`RawDataPatch`] (pushed onto `patches`, layer order preserved), anything
/// else as a full `R` row. Branching on the field FIRST keeps error messages
/// precise (an untagged enum would collapse both failure modes into "no
/// variant matched").
pub fn parse_rows_with_patches<R: serde::de::DeserializeOwned>(
    text: &str,
    array_key: &str,
    patches: &mut Vec<RawDataPatch>,
) -> Result<Vec<R>, serde_json::Error> {
    use serde::de::Error;
    let file: serde_json::Value = serde_json::from_str(text)?;
    let rows = file
        .get(array_key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| serde_json::Error::custom(format!("missing '{array_key}' array")))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.get("patch").is_some() {
            patches.push(serde_json::from_value(row.clone())?);
        } else {
            out.push(serde_json::from_value(row.clone())?);
        }
    }
    Ok(out)
}

/// Bounds on a row's `data` map — interop metadata, not bulk storage.
const DATA_KEYS_MAX: usize = 32;
const DATA_VALUE_MAX: usize = 4096;

/// Compile a row's `data` map — its own entries plus every [`RawDataPatch`]
/// targeting it, applied in layer order with later keys winning — into the
/// leaked sorted `(key, canonical JSON text)` slice the definition tables
/// hold. Keys must be namespaced (`ns:name`); values are OPAQUE raw JSON the
/// declaring pack writes in the CONSUMING mod's vocabulary (the item/block
/// interop surface — e.g. `"furniture:pigment"` on a berries item). The
/// engine validates only shape and bounds; a consumer parses what it
/// understands and ignores the rest.
pub fn compile_data_map(
    row_name: &str,
    base: &serde_json::Map<String, serde_json::Value>,
    patches: &[RawDataPatch],
) -> Result<&'static [(&'static str, &'static str)], String> {
    let mut merged: Vec<(String, String)> = Vec::new();
    let mut insert = |key: &str, value: &serde_json::Value| -> Result<(), String> {
        let ok_part = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_graphic());
        if !key
            .split_once(':')
            .is_some_and(|(ns, n)| ok_part(ns) && ok_part(n))
        {
            return Err(format!("data key '{key}' must be namespaced 'ns:name'"));
        }
        let text = value.to_string();
        if text.len() > DATA_VALUE_MAX {
            return Err(format!(
                "data value for '{key}' is {} bytes; the limit is {DATA_VALUE_MAX}",
                text.len()
            ));
        }
        match merged.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = text,
            None => merged.push((key.to_owned(), text)),
        }
        Ok(())
    };
    for (k, v) in base {
        insert(k, v)?;
    }
    for patch in patches.iter().filter(|p| p.patch == row_name) {
        for (k, v) in &patch.data {
            insert(k, v)?;
        }
    }
    if merged.len() > DATA_KEYS_MAX {
        return Err(format!(
            "{} data keys; the limit is {DATA_KEYS_MAX}",
            merged.len()
        ));
    }
    merged.sort_by(|a, b| a.0.cmp(&b.0));
    let leaked: Vec<(&'static str, &'static str)> = merged
        .into_iter()
        .map(|(k, v)| {
            (
                &*Box::leak(k.into_boxed_str()),
                &*Box::leak(v.into_boxed_str()),
            )
        })
        .collect();
    Ok(Box::leak(leaked.into_boxed_slice()))
}

fn parse_and_merge<R>(
    texts: &[&str],
    mut parse_layer: impl FnMut(&str) -> Result<Vec<R>, serde_json::Error>,
    row_key: fn(&R) -> &str,
) -> Result<(Vec<R>, Vec<Vec<String>>), String> {
    let mut merged: Vec<R> = Vec::new();
    let mut layer_keys: Vec<Vec<String>> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let rows = parse_layer(text).map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        layer_keys.push(rows.iter().map(|r| row_key(r).to_owned()).collect());
        for r in rows {
            match merged.iter().position(|m| row_key(m) == row_key(&r)) {
                Some(i) => merged[i] = r,
                None => merged.push(r),
            }
        }
    }
    Ok((merged, layer_keys))
}

fn resolve_merged<R, D>(
    merged: Vec<R>,
    row_key: fn(&R) -> &str,
    names: &NameTable,
    what: &str,
    mut convert: impl FnMut(R, u16, &NameTable) -> Result<D, String>,
) -> Result<Vec<D>, String> {
    let mut rows: Vec<Option<D>> = (0..names.len()).map(|_| None).collect();
    for r in merged {
        let id = names
            .id(row_key(&r))
            .ok_or_else(|| format!("unregistered {what} '{}'", row_key(&r)))?;
        rows[id as usize] = Some(convert(r, id, names)?);
    }
    rows.into_iter()
        .enumerate()
        .map(|(id, row)| {
            row.ok_or_else(|| {
                format!(
                    "missing row for {what} '{}'",
                    names.name(id as u16).unwrap_or("?")
                )
            })
        })
        .collect()
}

/// Parse an ENGINE consumer's entry out of a row's compiled data slice —
/// `petramond:fuel` / `petramond:tool` / `petramond:carry` are ordinary
/// data-surface consumers whose consuming system happens to be the engine
/// (the dogfooding rule). Absent = `None`; present-but-malformed = a load
/// error (the engine parses its own vocabulary strictly, unlike a mod key it
/// would ignore).
pub fn engine_data<T: serde::de::DeserializeOwned>(
    data: &'static [(&'static str, &'static str)],
    key: &str,
) -> Result<Option<T>, String> {
    let Some((_, text)) = data.iter().find(|(k, _)| *k == key) else {
        return Ok(None);
    };
    serde_json::from_str(text)
        .map(Some)
        .map_err(|e| format!("malformed '{key}' data: {e}"))
}

/// Validate an engine vocabulary entry that lists KV/instance-data keys
/// (`petramond:carry`, `petramond:inherit`): every listed key must be
/// namespaced. The one place the "no bare keys" rule for key-list entries
/// lives.
pub fn validate_namespaced_keys(what: &str, keys: &[String]) -> Result<(), String> {
    for k in keys {
        if namespace(k).is_none() {
            return Err(format!("{what} lists bare key '{k}'"));
        }
    }
    Ok(())
}

/// The catalog FILE frame around [`load_catalog`]/[`resolve_catalog`]: read
/// every layer of `file` (base assets + packs), then run `parse` over the
/// layer texts. These tables are load-bearing, so a missing file or a parse
/// error panics with a precise message instead of limping on.
pub fn read_catalog<T>(
    file: &str,
    what: &str,
    parse: impl FnOnce(&[&str]) -> Result<T, String>,
) -> T {
    read_catalog_labeled(file, what, |layers| {
        let texts: Vec<&str> = layers.iter().map(|(s, _)| *s).collect();
        parse(&texts)
    })
}

/// [`read_catalog`] with each layer's source path alongside its text, for the
/// catalogs whose diagnostics should name the pack a layer came from.
pub fn read_catalog_labeled<T>(
    file: &str,
    what: &str,
    parse: impl FnOnce(&[(&str, &std::path::Path)]) -> Result<T, String>,
) -> T {
    let layers = crate::assets::read_layers(file);
    if layers.is_empty() {
        panic!(
            "{file} not found (searched {:?}); the game cannot run without its {what} table",
            crate::assets::candidate_paths(file)
        );
    }
    for (_, path) in &layers {
        log::info!("{what} defs layer: {}", path.display());
    }
    let layers: Vec<(&str, &std::path::Path)> = layers
        .iter()
        .map(|(s, p)| (s.as_str(), p.as_path()))
        .collect();
    parse(&layers).unwrap_or_else(|e| panic!("{file}: {e}"))
}

/// Whether `key` carries a `namespace:` prefix.
pub fn is_namespaced(key: &str) -> bool {
    namespace(key).is_some()
}

/// The namespace of `key` (`"wheel:wheel" → Some("wheel")`,
/// `"petramond:stone" → Some("petramond")`), or `None` for bare and degenerate forms.
/// The per-world mod enablement gates (palette / recipes / natural spawner)
/// key off this.
pub fn namespace(key: &str) -> Option<&str> {
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
pub struct TagTable {
    engine: &'static [&'static str],
    dynamic: std::sync::RwLock<Vec<&'static str>>,
}

impl TagTable {
    pub const fn new(engine: &'static [&'static str]) -> Self {
        Self {
            engine,
            dynamic: std::sync::RwLock::new(Vec::new()),
        }
    }

    /// Resolve a tag name from data: a bare name must be an engine tag (typo
    /// guard — a misspelled engine tag must not silently become a new tag);
    /// `petramond:<engine>` resolves to the same id; a namespaced `mod_id:name`
    /// interns on first sight.
    pub fn resolve(&self, name: &str) -> Result<u8, String> {
        let bare = name.strip_prefix("petramond:").unwrap_or(name);
        if let Some(i) = self.engine.iter().position(|n| *n == bare) {
            return Ok(i as u8);
        }
        // The engine namespace is RESERVED, exactly as it is for content names
        // (see `NameTable::build`): without this, `petramond:leeves` interns a
        // brand-new tag nothing carries, so a typo in a row — or a pack
        // squatting an engine term — fails silently instead of loudly.
        if name.starts_with("petramond:") {
            return Err(format!(
                "unknown tag '{name}' — the 'petramond:' namespace is reserved for engine tags \
                 ({}); a mod tag must carry its own 'mod_id:' prefix",
                self.engine.join(", ")
            ));
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

    /// Look up an already-registered tag WITHOUT interning: engine names
    /// (bare or `petramond:`-prefixed) and previously-listed pack tags
    /// resolve; anything else is `None`. Queries must use this, never
    /// [`Self::resolve`] — a query for an arbitrary name must not be able to
    /// fill the 256-entry table.
    pub fn lookup(&self, name: &str) -> Option<u8> {
        let bare = name.strip_prefix("petramond:").unwrap_or(name);
        if let Some(i) = self.engine.iter().position(|n| *n == bare) {
            return Some(i as u8);
        }
        self.dynamic
            .read()
            .unwrap()
            .iter()
            .position(|n| *n == name)
            .map(|i| (self.engine.len() + i) as u8)
    }

    /// The registered name for `id` (diagnostics only).
    #[allow(dead_code)]
    pub fn name(&self, id: u8) -> &'static str {
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
pub struct ContentNames {
    pub blocks: NameTable,
    pub items: NameTable,
}

/// Build both tables from raw catalog layer texts — the pure core `names()`
/// wraps, split out so loader tests can drive it with synthetic layers. Only
/// the row KEYS are read here; full row validation stays with the loaders.
pub fn build_names(block_texts: &[&str], item_texts: &[&str]) -> Result<ContentNames, String> {
    // Key pre-parse only: rows with a `"patch"` field register nothing (they
    // attach data to an EXISTING row — see [`RawDataPatch`]), so they carry
    // no key here.
    fn layer_keys(
        texts: &[&str],
        file: &str,
        array_key: &str,
        key_field: &str,
    ) -> Result<Vec<Vec<String>>, String> {
        let mut out = Vec::new();
        for (li, text) in texts.iter().enumerate() {
            let err = |msg: String| format!("{file} layer #{li}: {msg}");
            let value: serde_json::Value =
                serde_json::from_str(text).map_err(|e| err(format!("invalid JSON: {e}")))?;
            let rows = value
                .get(array_key)
                .and_then(|v| v.as_array())
                .ok_or_else(|| err(format!("expected a top-level '{array_key}' array")))?;
            let mut keys = Vec::new();
            for row in rows {
                if row.get("patch").is_some() {
                    continue;
                }
                keys.push(
                    row.get(key_field)
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| err(format!("row has no string '{key_field}' key")))?
                        .to_owned(),
                );
            }
            out.push(keys);
        }
        Ok(out)
    }
    let block_keys = layer_keys(block_texts, "blocks.json", "blocks", "block")?;
    let item_keys = layer_keys(item_texts, "items.json", "items", "item")?;
    Ok(ContentNames {
        blocks: NameTable::build(
            crate::block::ENGINE_BLOCK_NAMES,
            &block_keys,
            "block",
            WIDE_ID_CAP,
        )?,
        items: NameTable::build(
            crate::item::ENGINE_ITEM_NAMES,
            &item_keys,
            "item",
            WIDE_ID_CAP,
        )?,
    })
}

/// The process-wide name tables, built once from the real catalog layers
/// (base `assets/` + packs). Loads on first touch from any thread; a bad pack
/// key fails loudly here, before any definition table builds on top of it.
pub fn names() -> &'static ContentNames {
    static NAMES: LazyLock<ContentNames> = LazyLock::new(|| {
        let blocks = crate::assets::read_layers("blocks.json");
        let items = crate::assets::read_layers("items.json");
        let block_texts: Vec<&str> = blocks.iter().map(|(s, _)| s.as_str()).collect();
        let item_texts: Vec<&str> = items.iter().map(|(s, _)| s.as_str()).collect();
        let names = build_names(&block_texts, &item_texts)
            .unwrap_or_else(|e| panic!("content registry: {e}"));
        // The ceiling is invisible until it is hit, and by then the only
        // signal is a refused pack. Say where the world is against it at every
        // boot, and say it LOUDLY once the remaining headroom is smaller than
        // an ordinary pack.
        for (what, used) in [("block", names.blocks.len()), ("item", names.items.len())] {
            let left = WIDE_ID_CAP - used;
            if left < ID_HEADROOM_WARN {
                log::warn!(
                    "{what} registry: {used}/{WIDE_ID_CAP} ids used, {left} left for further packs"
                );
            } else {
                log::info!("{what} registry: {used}/{WIDE_ID_CAP} ids used");
            }
        }
        names
    });
    &NAMES
}

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    #[allow(unused_imports)]
    pub use super::*;
}
