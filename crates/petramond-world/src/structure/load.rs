use serde::Deserialize;

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

/// The structure-template catalog stage of every content registry. Templates
/// name blocks, so it builds after the shared name tables; every template
/// compiles at load, including ones worldgen never selects.
pub(crate) static CATALOG: crate::content::Slot<Catalog<Template>> = crate::content::Slot::new(
    crate::content::stage::STRUCTURES,
    &[crate::content::stage::NAMES],
    load,
);

fn load(reg: &crate::content::ContentRegistry) -> Result<Catalog<Template>, String> {
    let packs = reg.packs();
    crate::registry::read_catalog(packs, "structures.json", "structure", |texts| {
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
                let (bytes, _) = packs.read_bytes(&row.file).ok_or_else(|| {
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
                    reg.names().blocks.id(key).map(crate::block::Block)
                })
                .map_err(|e| format!("structure '{}' ({}): {e}", row.structure, row.file))
            },
        )
    })
}

/// Resolve a namespaced template of the current registry.
pub fn by_key(key: &str) -> Option<&'static Template> {
    let catalog = CATALOG.current();
    catalog.id(key).map(|id| &catalog.rows()[id as usize])
}
