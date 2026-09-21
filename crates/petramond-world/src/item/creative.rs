//! Portable creative items for registered blocks without an ordinary item.
use std::collections::{HashMap, HashSet};
pub(crate) const PREFIX: &str = "petramond:creative/";

pub(crate) fn item_name(block: &str) -> String {
    format!("{PREFIX}{block}")
}

pub(crate) fn missing(
    blocks: &crate::registry::NameTable,
    texts: &[&str],
) -> Result<Vec<String>, String> {
    let mut links = HashMap::new();
    for text in texts {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        for row in value["items"].as_array().into_iter().flatten() {
            if let Some(item) = row["item"].as_str() {
                links.insert(item.to_owned(), row["block"].as_str().map(str::to_owned));
            }
        }
    }
    let covered: HashSet<_> = links.into_values().flatten().collect();
    Ok((1..blocks.len())
        .filter_map(|id| {
            let name = blocks.name(id as u16).unwrap();
            (!covered.contains(name)).then(|| item_name(name))
        })
        .collect())
}

pub(super) fn catalog(names: &crate::registry::ContentNames) -> String {
    let rows: Vec<_> = (0..names.items.len())
        .filter_map(|id| {
            let item = names.items.name(id as u16).expect("dense item names");
            let block = item.strip_prefix(PREFIX)?;
            let title = block
                .split_once(':')
                .map_or(block, |(_, s)| s)
                .replace('_', " ");
            Some(serde_json::json!({
                "item": item,
                "key": item,
                "name": title,
                "max_stack_size": 64,
                "block": block,
                "data": {"petramond:creative_only": true}
            }))
        })
        .collect();
    serde_json::json!({"items":rows}).to_string()
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct CatalogEntry {
    pub visible: Option<bool>,
    pub name: Option<String>,
    pub placement_variants: Vec<String>,
}

pub(super) fn resolve(
    data: &'static [(&'static str, &'static str)],
    key: &str,
    block: Option<crate::block::Block>,
    names: &crate::registry::ContentNames,
) -> Result<(bool, Option<String>, &'static [crate::block::Block]), String> {
    let entry = crate::registry::engine_data::<CatalogEntry>(data, "petramond:creative")?
        .unwrap_or_default();
    if entry.name.as_ref().is_some_and(|s| s.trim().is_empty()) {
        return Err("creative name must not be empty".into());
    }
    if !entry.placement_variants.is_empty() && block.is_none() {
        return Err("placement variants require a block item".into());
    }
    let mut seen = std::collections::HashSet::new();
    let variants = entry
        .placement_variants
        .iter()
        .map(|name| {
            let id = names
                .blocks
                .id(name)
                .filter(|id| *id != 0)
                .ok_or_else(|| format!("unknown placement variant '{name}'"))?;
            if !seen.insert(id) {
                return Err(format!("duplicate placement variant '{name}'"));
            }
            Ok(crate::block::Block(id))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((
        entry.visible.unwrap_or(!key.starts_with(PREFIX)),
        entry.name,
        Box::leak(variants.into_boxed_slice()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_metadata_opts_generated_items_in_and_validates_variants() {
        let names = crate::registry::build_names(
            &[r#"{"blocks":[{"block":"fixture:branch"},{"block":"fixture:twig"}]}"#],
            &[],
        )
        .unwrap();
        let block = Some(crate::block::Block(
            names.blocks.id("fixture:branch").unwrap(),
        ));
        assert!(
            !resolve(&[], "petramond:creative/fixture:branch", block, &names)
                .unwrap()
                .0
        );
        assert!(
            resolve(&[], "fixture:branch_item", block, &names)
                .unwrap()
                .0
        );
        let data = &[(
            "petramond:creative",
            r#"{"visible":true,"name":"Branch","placement_variants":["fixture:branch","fixture:twig"]}"#,
        )];
        let entry = resolve(data, "petramond:creative/fixture:branch", block, &names).unwrap();
        assert!(entry.0);
        assert_eq!(entry.1.as_deref(), Some("Branch"));
        assert_eq!(entry.2.len(), 2);
        assert!(resolve(data, "fixture:tool", None, &names).is_err());
        assert!(resolve(
            &[(
                "petramond:creative",
                r#"{"placement_variants":["fixture:missing"]}"#
            )],
            "fixture:branch",
            block,
            &names
        )
        .is_err());
        assert!(resolve(
            &[(
                "petramond:creative",
                r#"{"placement_variants":["fixture:branch","fixture:branch"]}"#
            )],
            "fixture:branch",
            block,
            &names
        )
        .is_err());
    }
}
