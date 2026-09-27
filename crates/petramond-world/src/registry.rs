use std::collections::HashMap;

use serde::Deserialize;

pub const ENGINE_NAMESPACE: &str = "petramond";

pub const WIDE_ID_CAP: usize = 4096;

pub const ID_HEADROOM_WARN: usize = 128;

pub const BYTE_ID_CAP: usize = 256;

#[derive(Debug)]
pub struct NameTable {
    names: Vec<&'static str>,
    ids: HashMap<&'static str, u16>,
}

impl NameTable {
    pub fn id(&self, name: &str) -> Option<u16> {
        self.ids.get(name).copied()
    }

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

    /// Builds the table from the engine names, then each layer's keys in order. An engine name
    /// or an already-registered name is an override and gets no new id. A namespaced key
    /// outside `petramond` (`mod_id:name`) gets the next id. Bare keys and unknown
    /// `petramond:*` keys are errors. `cap` is the same id ceiling `modding::manifest` uses.
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
                    continue;
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

pub struct Catalog<D: 'static> {
    rows: &'static [D],
    names: NameTable,
}

impl<D> Catalog<D> {
    pub fn rows(&self) -> &'static [D] {
        self.rows
    }

    pub fn id(&self, name: &str) -> Option<u16> {
        self.names.id(name)
    }
}

/// Shared load path behind `effects.json`, `sounds.json`, `models.json`, `blocks.json` and so on.
/// Rows merge by key and a later layer's row replaces the earlier one, so a pack only writes the
/// rows it changes or adds. Engine names keep their frozen ids and namespaced keys follow in load
/// order (see [`NameTable::build`]). Every merged row goes through `convert`, and the result must
/// be dense: each name exactly once, no id gaps.
pub fn load_catalog<R, D>(
    texts: &[&str],
    parse_layer: impl FnMut(&str) -> Result<Vec<R>, serde_json::Error>,
    row_key: fn(&R) -> &str,
    engine: &[&'static str],
    what: &str,
    convert: impl FnMut(R, u16, &NameTable) -> Result<D, String>,
) -> Result<Catalog<D>, String> {
    load_catalog_with_capacity(
        texts,
        parse_layer,
        row_key,
        engine,
        what,
        BYTE_ID_CAP,
        convert,
    )
}

pub fn load_catalog_with_capacity<R, D>(
    texts: &[&str],
    parse_layer: impl FnMut(&str) -> Result<Vec<R>, serde_json::Error>,
    row_key: fn(&R) -> &str,
    engine: &[&'static str],
    what: &str,
    capacity: usize,
    convert: impl FnMut(R, u16, &NameTable) -> Result<D, String>,
) -> Result<Catalog<D>, String> {
    let (merged, layer_keys) = parse_and_merge(texts, parse_layer, row_key)?;
    let names = NameTable::build(engine, &layer_keys, what, capacity)?;
    let rows = resolve_merged(merged, row_key, &names, what, convert)?;
    Ok(Catalog {
        rows: Box::leak(rows.into_boxed_slice()),
        names,
    })
}

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawDataPatch {
    pub patch: String,
    pub data: serde_json::Map<String, serde_json::Value>,
}

/// Expands one layer's `"extends"` rows in place. A row that extends an earlier row (matched by
/// `key_field`) starts as a copy of it, and any field it writes replaces the base's whole field.
/// Lists and maps don't deep merge. It keeps its own key and `extends` is stripped before serde.
/// Chains are fine. Only earlier rows in the same layer can be a base, so sixteen rail forms can
/// be one full row plus fifteen deltas without depending on another layer.
fn expand_extends(
    rows: &mut [serde_json::Value],
    key_field: &str,
) -> Result<(), serde_json::Error> {
    use serde::de::Error;
    for i in 0..rows.len() {
        let Some(base_key) = rows[i].get("extends").cloned() else {
            continue;
        };
        let Some(base_key) = base_key.as_str() else {
            return Err(Error::custom(format!(
                "row #{i}: `extends` must name a row of this layer"
            )));
        };
        let base = rows[..i]
            .iter()
            .find(|r| r.get(key_field).and_then(|k| k.as_str()) == Some(base_key))
            .and_then(|r| r.as_object().cloned())
            .ok_or_else(|| {
                Error::custom(format!(
                    "row #{i} extends '{base_key}', which is not an earlier row of this layer"
                ))
            })?;
        let Some(row) = rows[i].as_object() else {
            return Err(Error::custom(format!("row #{i} is not an object")));
        };
        let mut merged = base;
        merged.extend(
            row.iter()
                .filter(|(k, _)| *k != "extends")
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        rows[i] = serde_json::Value::Object(merged);
    }
    Ok(())
}

fn layer_rows(
    file: &serde_json::Value,
    array_key: &str,
    key_field: &str,
) -> Result<Vec<serde_json::Value>, serde_json::Error> {
    use serde::de::Error;
    let mut rows = file
        .get(array_key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| serde_json::Error::custom(format!("missing '{array_key}' array")))?
        .clone();
    expand_extends(&mut rows, key_field)?;
    Ok(rows)
}

pub fn parse_rows<R: serde::de::DeserializeOwned>(
    text: &str,
    array_key: &str,
    key_field: &str,
) -> Result<Vec<R>, serde_json::Error> {
    parse_rows_of(&serde_json::from_str(text)?, array_key, key_field)
}

pub fn parse_rows_of<R: serde::de::DeserializeOwned>(
    file: &serde_json::Value,
    array_key: &str,
    key_field: &str,
) -> Result<Vec<R>, serde_json::Error> {
    layer_rows(file, array_key, key_field)?
        .into_iter()
        .map(serde_json::from_value)
        .collect()
}

pub fn parse_rows_with_patches<R: serde::de::DeserializeOwned>(
    text: &str,
    array_key: &str,
    key_field: &str,
    patches: &mut Vec<RawDataPatch>,
) -> Result<Vec<R>, serde_json::Error> {
    parse_rows_with_patches_of(&serde_json::from_str(text)?, array_key, key_field, patches)
}

pub fn parse_rows_with_patches_of<R: serde::de::DeserializeOwned>(
    file: &serde_json::Value,
    array_key: &str,
    key_field: &str,
    patches: &mut Vec<RawDataPatch>,
) -> Result<Vec<R>, serde_json::Error> {
    let rows = layer_rows(file, array_key, key_field)?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.get("patch").is_some() {
            patches.push(serde_json::from_value(row)?);
        } else {
            out.push(serde_json::from_value(row)?);
        }
    }
    Ok(out)
}

const DATA_KEYS_MAX: usize = 32;
const DATA_VALUE_MAX: usize = 4096;

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
    let mut errors: Vec<String> = Vec::new();
    for r in merged {
        let Some(id) = names.id(row_key(&r)) else {
            errors.push(format!("unregistered {what} '{}'", row_key(&r)));
            continue;
        };
        match convert(r, id, names) {
            Ok(row) => rows[id as usize] = Some(row),
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
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

pub const ENABLED_KEY: &str = "petramond:enabled";

pub fn row_enabled<K: AsRef<str>, V: AsRef<str>>(data: &[(K, V)]) -> Result<bool, String> {
    match data.iter().find(|(k, _)| k.as_ref() == ENABLED_KEY) {
        None => Ok(true),
        Some((_, text)) => serde_json::from_str(text.as_ref())
            .map_err(|e| format!("malformed '{ENABLED_KEY}' data: {e}")),
    }
}

pub fn validate_namespaced_keys(what: &str, keys: &[String]) -> Result<(), String> {
    for k in keys {
        if namespace(k).is_none() {
            return Err(format!("{what} lists bare key '{k}'"));
        }
    }
    Ok(())
}

pub fn read_catalog<T>(
    packs: &crate::assets::PackSet,
    file: &str,
    what: &str,
    parse: impl FnOnce(&[&str]) -> Result<T, String>,
) -> Result<T, String> {
    read_catalog_labeled(packs, file, what, |layers| {
        let texts: Vec<&str> = layers.iter().map(|(s, _)| *s).collect();
        parse(&texts)
    })
}

pub fn read_asset_catalog<T>(
    packs: &crate::assets::PackSet,
    file: &str,
    what: &str,
    parse: impl FnOnce(&[&str]) -> Result<T, String>,
) -> Result<T, String> {
    parse_catalog_layers(packs, packs.read_asset_layers(file), file, what, |layers| {
        let texts: Vec<&str> = layers.iter().map(|(s, _)| *s).collect();
        parse(&texts)
    })
}

pub fn read_catalog_labeled<T>(
    packs: &crate::assets::PackSet,
    file: &str,
    what: &str,
    parse: impl FnOnce(&[(&str, &std::path::Path)]) -> Result<T, String>,
) -> Result<T, String> {
    parse_catalog_layers(packs, packs.read_layers(file), file, what, parse)
}

fn parse_catalog_layers<T>(
    packs: &crate::assets::PackSet,
    layers: Vec<(String, std::path::PathBuf)>,
    file: &str,
    what: &str,
    parse: impl FnOnce(&[(&str, &std::path::Path)]) -> Result<T, String>,
) -> Result<T, String> {
    if layers.is_empty() {
        return Err(format!(
            "{file} not found (searched {:?}); the game cannot run without its {what} table",
            packs.candidate_paths(file)
        ));
    }
    for (_, path) in &layers {
        log::info!("{what} defs layer: {}", path.display());
    }
    let layers: Vec<(&str, &std::path::Path)> = layers
        .iter()
        .map(|(s, p)| (s.as_str(), p.as_path()))
        .collect();
    parse(&layers)
}

pub fn is_namespaced(key: &str) -> bool {
    namespace(key).is_some()
}

pub fn namespace(key: &str) -> Option<&str> {
    match key.split_once(':') {
        Some((ns, name)) if !ns.is_empty() && !name.is_empty() => Some(ns),
        _ => None,
    }
}

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

    pub fn resolve(&self, name: &str) -> Result<u8, String> {
        let bare = name.strip_prefix("petramond:").unwrap_or(name);
        if let Some(i) = self.engine.iter().position(|n| *n == bare) {
            return Ok(i as u8);
        }
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

pub struct ContentNames {
    pub blocks: NameTable,
    pub items: NameTable,
}

pub fn build_names(block_texts: &[&str], item_texts: &[&str]) -> Result<ContentNames, String> {
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
    let blocks = NameTable::build(
        crate::block::ENGINE_BLOCK_NAMES,
        &block_keys,
        "block",
        WIDE_ID_CAP,
    )?;
    let mut items = NameTable::build(
        crate::item::ENGINE_ITEM_NAMES,
        &item_keys,
        "item",
        WIDE_ID_CAP,
    )?;
    let creative_items = crate::item::creative::missing(&blocks, item_texts)?;
    if items.len() + creative_items.len() > WIDE_ID_CAP {
        return Err("Item registry is full, including creative block items".into());
    }
    for name in creative_items {
        items.push(String::leak(name));
    }
    Ok(ContentNames { blocks, items })
}

pub fn load_names(packs: &crate::assets::PackSet) -> Result<ContentNames, String> {
    let blocks = packs.read_layers("blocks.json");
    let items = packs.read_layers("items.json");
    let block_texts: Vec<&str> = blocks.iter().map(|(s, _)| s.as_str()).collect();
    let item_texts: Vec<&str> = items.iter().map(|(s, _)| s.as_str()).collect();
    let names = build_names(&block_texts, &item_texts)?;
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
    Ok(names)
}

#[inline]
pub fn names() -> &'static ContentNames {
    crate::content::current().names()
}

#[cfg(test)]
mod tests;
