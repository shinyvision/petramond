use serde::Deserialize;
use std::sync::LazyLock;

use super::Template;
use crate::registry::Catalog;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    structures: Vec<Row>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    structure: String,
    file: String,
}

static CATALOG: LazyLock<Catalog<Template>> = LazyLock::new(|| {
    crate::registry::read_catalog("structures.json", "structure", |texts| {
        crate::registry::load_catalog(
            texts,
            |text| serde_json::from_str::<File>(text).map(|file| file.structures),
            |row| &row.structure,
            &[],
            "structure",
            |row, _, _| {
                let path = std::path::Path::new(&row.file);
                if !row.file.starts_with("structures/")
                    || !row.file.ends_with(".json")
                    || path
                        .components()
                        .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(format!(
                        "structure '{}': expected relative structures/*.json path",
                        row.structure
                    ));
                }
                let (bytes, _) = crate::assets::read_bytes(&row.file).ok_or_else(|| {
                    format!("structure '{}': missing '{}'", row.structure, row.file)
                })?;
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err(format!(
                        "structure '{}': asset exceeds 4 MiB",
                        row.structure
                    ));
                }
                let json = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
                Template::parse(json, |key| {
                    crate::registry::names()
                        .blocks
                        .id(key)
                        .map(crate::block::Block)
                })
                .map_err(|e| format!("structure '{}' ({}): {e}", row.structure, row.file))
            },
        )
    })
});

/// Resolve a namespaced template after pack admission, compiled once.
pub fn by_key(key: &str) -> Option<&'static Template> {
    CATALOG.id(key).map(|id| &CATALOG.rows()[id as usize])
}

/// Force validation of every template, including ones not selected by worldgen.
pub fn validate_catalog() {
    let _ = CATALOG.rows();
}
