//! Load layered recipe data from `recipes.json`.
//!
//! Player crafting has one quantity-based format. Processing rows retain
//! their separate schema because machines consume them by a different
//! interaction model.

use serde::Deserialize;

use crate::item::{ItemStack, ItemTag, ItemType};

use super::recipe::{
    CraftingIngredient, CraftingRecipe, IngredientSelector, IngredientUse, ProcessingRecipe,
    Recipes,
};
use super::station::CraftingStation;
use crate::assets::CatalogLayer;

const EMBEDDED: &str = include_str!("../../../../assets/recipes.json");

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawRecipe {
    Crafting {
        recipe: String,
        station: String,
        ingredients: Vec<RawCraftingIngredient>,
        result: RawStack,
        #[serde(default)]
        data: serde_json::Map<String, serde_json::Value>,
    },
    Processing {
        recipe: String,
        class: String,
        ingredient: String,
        result: String,
        #[serde(default = "one_u8")]
        count: u8,
        #[serde(default)]
        data: serde_json::Map<String, serde_json::Value>,
    },
}

#[derive(Deserialize)]
struct RawStack {
    item: String,
    count: u8,
}

#[derive(Deserialize)]
struct RawCraftingIngredient {
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    count: u16,
    #[serde(default)]
    keep: bool,
    #[serde(default)]
    remainder: Option<String>,
}

enum Converted {
    /// A row plus its own raw `data` map — compiled AFTER all layers parse, so
    /// patch rows from later packs can target it.
    Crafting(CraftingRecipe, serde_json::Map<String, serde_json::Value>),
    Processing(ProcessingRecipe, serde_json::Map<String, serde_json::Value>),
}

fn one_u8() -> u8 {
    1
}

/// Load base + enabled pack recipe layers in deterministic pack order.
///
/// Layer ownership is retained: disabling a pack removes its whole layer even
/// when one of its rows mentions engine content only, and an integration
/// layer goes with EITHER pack it joins. Reference filtering then removes
/// enabled/base rows that touch another disabled namespace.
pub fn load_recipes_for(disabled: &std::collections::BTreeSet<String>) -> Recipes {
    load_layers(read_recipe_layers(), disabled)
}

fn load_layers(
    layers: impl IntoIterator<Item = CatalogLayer>,
    disabled: &std::collections::BTreeSet<String>,
) -> Recipes {
    let mut crafting: Vec<(CraftingRecipe, serde_json::Map<String, serde_json::Value>)> =
        Vec::new();
    let mut processing = Vec::new();
    let mut patches = Vec::new();
    for CatalogLayer {
        text,
        path,
        owner,
        requires,
    } in layers
    {
        if let Some(id) = requires.iter().find(|id| disabled.contains(*id)) {
            log::info!(
                "skipping recipes layer {}: mod '{id}' is disabled",
                path.display()
            );
            continue;
        }
        log::info!("crafting recipes layer: {}", path.display());
        let (c, p) = parse_for(&text, disabled, owner.as_deref(), &mut patches);
        crafting.extend(c);
        processing.extend(p);
    }
    // Compile each row's data map now that every layer's patch rows are in (a
    // patch targets the FINAL row, cross-namespace by design). BOTH row kinds
    // go through the same gate: a recipe is a recipe, and a pack retires one
    // the same way whichever machine consumes it.
    let mut retired: Vec<String> = Vec::new();
    let mut compiled = Vec::with_capacity(crafting.len());
    for (mut recipe, own) in crafting {
        let Some(entries) = compile_row(recipe.key(), &own, &patches, &mut retired) else {
            continue;
        };
        if let Err(error) = recipe.set_data(entries) {
            log::error!("skipping recipe '{}': {error}", recipe.key());
            continue;
        }
        compiled.push(recipe);
    }
    let mut compiled_processing = Vec::with_capacity(processing.len());
    for (recipe, own) in processing {
        if compile_row(&recipe.key, &own, &patches, &mut retired).is_some() {
            compiled_processing.push(recipe);
        }
    }
    for patch in &patches {
        // A patch that RETIRED its target is the one case where the target is
        // legitimately absent from the catalog.
        let known = compiled.iter().any(|r| r.key() == patch.patch)
            || compiled_processing.iter().any(|r| r.key == patch.patch)
            || retired.contains(&patch.patch);
        if !known {
            log::error!("recipe patch targets unknown recipe '{}'", patch.patch);
        }
    }
    Recipes::new(compiled, compiled_processing)
}

/// Merge one row's own `data` map with every patch targeting it and apply the
/// engine's `petramond:enabled` gate. `None` = the row does not join the
/// catalog (retired, or malformed data).
///
/// The merged map is READ and dropped: `petramond:enabled` is the only key the
/// engine understands on a recipe row. Everything else a pack writes there is
/// carried for the patch mechanism's sake — a pack retires an engine recipe by
/// patching a row it does not own — and is deliberately not surfaced to the
/// compiled recipe.
fn compile_row(
    key: &str,
    own: &serde_json::Map<String, serde_json::Value>,
    patches: &[crate::registry::RawDataPatch],
    retired: &mut Vec<String>,
) -> Option<Vec<(String, String)>> {
    let entries: Vec<(String, String)> = match crate::registry::compile_data_map(key, own, patches)
    {
        Ok(slice) => slice
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        Err(error) => {
            log::error!("skipping recipe '{key}': {error}");
            return None;
        }
    };
    match CraftingRecipe::row_enabled(&entries) {
        Ok(true) => Some(entries),
        Ok(false) => {
            log::info!("recipe '{key}' is retired by row data");
            retired.push(key.to_owned());
            None
        }
        Err(error) => {
            log::error!("skipping recipe '{key}': {error}");
            None
        }
    }
}

fn read_recipe_layers() -> Vec<CatalogLayer> {
    let layers = crate::assets::read_catalog_layers("recipes.json");
    if layers.is_empty() {
        log::info!("crafting recipes: no on-disk recipes.json found, using embedded defaults");
        vec![CatalogLayer {
            text: EMBEDDED.to_owned(),
            path: std::path::PathBuf::from("<embedded recipes.json>"),
            owner: None,
            requires: Vec::new(),
        }]
    } else {
        layers
    }
}

#[cfg(test)]
fn parse(text: &str) -> (Vec<CraftingRecipe>, Vec<ProcessingRecipe>) {
    let mut patches = Vec::new();
    let (crafting, processing) =
        parse_for(text, &std::collections::BTreeSet::new(), None, &mut patches);
    (
        crafting.into_iter().map(|(r, _)| r).collect(),
        processing.into_iter().map(|(r, _)| r).collect(),
    )
}

#[allow(clippy::type_complexity)]
fn parse_for(
    text: &str,
    disabled: &std::collections::BTreeSet<String>,
    owner: Option<&str>,
    patches: &mut Vec<crate::registry::RawDataPatch>,
) -> (
    Vec<(CraftingRecipe, serde_json::Map<String, serde_json::Value>)>,
    Vec<(ProcessingRecipe, serde_json::Map<String, serde_json::Value>)>,
) {
    // Patch rows (`{"patch", "data"}`) split out before the tagged-enum
    // parse, exactly like items/blocks; they register nothing and are exempt
    // from the owner gate (cross-namespace attach is the point).
    let rows: Vec<RawRecipe> =
        match crate::registry::parse_rows_with_patches(text, "recipes", patches) {
            Ok(rows) => rows,
            Err(error) => {
                log::error!("recipes.json is not valid JSON: {error}");
                return (Vec::new(), Vec::new());
            }
        };
    let mut crafting = Vec::new();
    let mut processing = Vec::new();
    for (index, raw) in rows.into_iter().enumerate() {
        if let Some(namespace) = disabled_namespace_in(&raw, disabled) {
            log::info!("skipping recipe #{index}: it references disabled mod '{namespace}'");
            continue;
        }
        match convert(raw, owner) {
            Ok(Converted::Crafting(recipe, data)) => crafting.push((recipe, data)),
            Ok(Converted::Processing(recipe, data)) => processing.push((recipe, data)),
            Err(error) => log::error!("skipping recipe #{index}: {error}"),
        }
    }
    (crafting, processing)
}

fn convert(raw: RawRecipe, owner: Option<&str>) -> Result<Converted, String> {
    match raw {
        RawRecipe::Crafting {
            recipe,
            station,
            ingredients,
            result,
            data,
        } => {
            validate_recipe_owner("crafting recipe", &recipe, owner)?;
            let station = CraftingStation::from_key(&station)
                .ok_or_else(|| format!("unknown crafting station '{station}'"))?;
            let ingredients: Vec<CraftingIngredient> = ingredients
                .into_iter()
                .map(convert_ingredient)
                .collect::<Result<_, _>>()?;
            let result_item = resolve_item(&result.item)?;
            Ok(Converted::Crafting(
                CraftingRecipe::try_new(
                    recipe,
                    station,
                    ingredients,
                    ItemStack::new(result_item, result.count),
                )?,
                data,
            ))
        }
        RawRecipe::Processing {
            recipe,
            class,
            ingredient,
            result,
            count,
            data,
        } => {
            validate_recipe_owner("processing recipe", &recipe, owner)?;
            if !crate::registry::is_namespaced(&class) {
                return Err(format!("processing class '{class}' is not namespaced"));
            }
            let input = resolve_item(&ingredient)?;
            let result = resolve_item(&result)?;
            validate_stack_count(result, count, "processing result")?;
            Ok(Converted::Processing(
                ProcessingRecipe {
                    key: recipe,
                    class,
                    input,
                    result: ItemStack::new(result, count),
                },
                data,
            ))
        }
    }
}

fn convert_ingredient(raw: RawCraftingIngredient) -> Result<CraftingIngredient, String> {
    if raw.count == 0 {
        return Err("crafting ingredient count is zero".into());
    }
    let selector = match (raw.item, raw.tag) {
        (Some(item), None) => IngredientSelector::Item(resolve_item(&item)?),
        (None, Some(tag)) => IngredientSelector::Tag(
            ItemTag::resolve(&tag).map_err(|error| format!("unknown item tag '{tag}': {error}"))?,
        ),
        (Some(_), Some(_)) => {
            return Err("crafting ingredient declares both 'item' and 'tag'".into())
        }
        (None, None) => return Err("crafting ingredient declares neither 'item' nor 'tag'".into()),
    };
    let use_mode = match (raw.keep, raw.remainder) {
        (true, Some(_)) => return Err("ingredient cannot be kept and return a remainder".into()),
        (true, None) => IngredientUse::Keep,
        (false, Some(remainder)) => IngredientUse::Remainder(resolve_item(&remainder)?),
        (false, None) => IngredientUse::Consume,
    };
    Ok(CraftingIngredient {
        selector,
        count: raw.count,
        use_mode,
    })
}

/// Both recipe kinds go through this, so `kind` names which one the author is
/// actually looking at — "crafting recipe" on a malformed processing key sends
/// them to the wrong file.
fn validate_recipe_owner(kind: &str, key: &str, owner: Option<&str>) -> Result<(), String> {
    let namespace = crate::registry::namespace(key)
        .ok_or_else(|| format!("{kind} key '{key}' is not namespaced"))?;
    match owner {
        Some(owner) if namespace == owner => Ok(()),
        Some(owner) => Err(format!(
            "{kind} key '{key}' does not belong to pack '{owner}'"
        )),
        None if namespace == crate::registry::ENGINE_NAMESPACE => Ok(()),
        None => Err(format!("{kind} key '{key}' ships without its owning pack")),
    }
}

fn resolve_item(key: &str) -> Result<ItemType, String> {
    ItemType::by_key(key).ok_or_else(|| format!("unknown item '{key}'"))
}

fn validate_stack_count(item: ItemType, count: u8, what: &str) -> Result<(), String> {
    if count == 0 || count > item.max_stack_size() {
        Err(format!(
            "{what} count {count} does not fit one '{}' stack (max {})",
            item.key(),
            item.max_stack_size()
        ))
    } else {
        Ok(())
    }
}

fn disabled_namespace_in<'a>(
    raw: &RawRecipe,
    disabled: &'a std::collections::BTreeSet<String>,
) -> Option<&'a str> {
    let hit = |key: &str| -> Option<&'a str> {
        crate::registry::namespace(key)
            .and_then(|namespace| disabled.get(namespace).map(String::as_str))
    };
    match raw {
        RawRecipe::Crafting {
            recipe,
            station,
            ingredients,
            result,
            ..
        } => hit(recipe)
            .or_else(|| hit(station))
            .or_else(|| {
                ingredients.iter().find_map(|ingredient| {
                    ingredient
                        .item
                        .as_deref()
                        .and_then(hit)
                        .or_else(|| ingredient.tag.as_deref().and_then(hit))
                        .or_else(|| ingredient.remainder.as_deref().and_then(hit))
                })
            })
            .or_else(|| hit(&result.item)),
        RawRecipe::Processing {
            recipe,
            class,
            ingredient,
            result,
            ..
        } => hit(recipe)
            .or_else(|| hit(class))
            .or_else(|| hit(ingredient))
            .or_else(|| hit(result)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(text: &str, path: &str, owner: Option<&str>, requires: &[&str]) -> CatalogLayer {
        CatalogLayer {
            text: text.to_owned(),
            path: std::path::PathBuf::from(path),
            owner: owner.map(str::to_owned),
            requires: requires.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// A pack's integration with another retires the pack's OWN route in
    /// favour of the other's. That layer must follow the TARGET, not just its
    /// owner: with the target switched off for a world, the plain route is
    /// the only one left and the retirement must not land.
    #[test]
    fn an_integration_layer_is_dropped_with_either_mod_it_joins() {
        let own = r#"{ "recipes": [{
            "type":"crafting","recipe":"combat:test_blade","station":"petramond:inventory",
            "ingredients":[{"item":"petramond:iron_ingot","count":2}],
            "result":{"item":"petramond:iron_pickaxe","count":1}
        }] }"#;
        let bridge = r#"{ "recipes": [
            {"patch":"combat:test_blade","data":{"petramond:enabled":false}}
        ] }"#;
        let loaded = |disabled: &[&str]| {
            let disabled = disabled.iter().map(|s| (*s).to_owned()).collect();
            load_layers(
                vec![
                    layer(own, "combat/recipes.json", Some("combat"), &["combat"]),
                    layer(
                        bridge,
                        "combat/integrations/forge/recipes.json",
                        Some("combat"),
                        &["combat", "forge"],
                    ),
                ],
                &disabled,
            )
        };
        assert!(loaded(&[]).crafting().get("combat:test_blade").is_none());
        assert!(loaded(&["forge"])
            .crafting()
            .get("combat:test_blade")
            .is_some());
        assert!(loaded(&["combat"])
            .crafting()
            .get("combat:test_blade")
            .is_none());
    }

    #[test]
    fn shipped_catalog_parses_both_interaction_models() {
        let (crafting, processing) = parse(EMBEDDED);
        assert!(!crafting.is_empty());
        assert!(!processing.is_empty());
        assert!(crafting
            .iter()
            .all(|recipe| !recipe.ingredients().is_empty()));
    }

    #[test]
    fn crafting_schema_resolves_quantities_tags_and_remainders() {
        let text = r#"{ "recipes": [{
            "type":"crafting", "recipe":"petramond:test", "station":"petramond:inventory",
            "ingredients":[
                {"tag":"petramond:planks","count":2},
                {"item":"petramond:water_bucket","count":1,"remainder":"petramond:wooden_bucket"}
            ],
            "result":{"item":"petramond:stick","count":4}
        }] }"#;
        let (crafting, _) = parse(text);
        let recipe = crafting.first().expect("valid recipe");
        assert_eq!(recipe.key(), "petramond:test");
        assert_eq!(recipe.ingredients()[0].count, 2);
        assert_eq!(
            recipe.ingredients()[0].selector,
            IngredientSelector::Tag(ItemTag::PLANKS)
        );
        assert_eq!(
            recipe.ingredients()[1].use_mode,
            IngredientUse::Remainder(ItemType::WoodenBucket)
        );
    }

    #[test]
    fn malformed_crafting_rows_are_skipped_without_legacy_decoders() {
        let text = r#"{ "recipes": [
            {"type":"crafting","recipe":"petramond:ok","station":"petramond:inventory",
             "ingredients":[{"item":"petramond:oak_log","count":1}],
             "result":{"item":"petramond:oak_planks","count":4}},
            {"type":"crafting","recipe":"petramond:both","station":"petramond:inventory",
             "ingredients":[{"item":"petramond:oak_log","tag":"petramond:logs","count":1}],
             "result":{"item":"petramond:oak_planks","count":4}},
            {"type":"crafting","recipe":"petramond:free","station":"petramond:inventory",
             "ingredients":[{"item":"petramond:stone_shovel","count":1,"keep":true}],
             "result":{"item":"petramond:stick","count":1}}
        ] }"#;
        let (crafting, _) = parse(text);
        assert_eq!(crafting.len(), 1);

        let legacy = r#"{ "recipes": [{"type":"shapeless","ingredients":["petramond:oak_log"],"result":"petramond:oak_planks"}] }"#;
        assert!(parse(legacy).0.is_empty());
        // The retired furniture row shape is malformed, not decoded.
        let furniture = r#"{ "recipes": [{"type":"furniture","input":"petramond:oak_planks","result":"petramond:oak_door","cost":1}] }"#;
        let (crafting, processing) = parse(furniture);
        assert!(crafting.is_empty() && processing.is_empty());
    }

    #[test]
    fn processing_lookup_contract_remains_distinct() {
        let text = r#"{ "recipes": [
            {"type":"processing","recipe":"petramond:test_cook","class":"test:cooking","ingredient":"petramond:raw_iron","result":"petramond:iron_ingot"}
        ] }"#;
        let (crafting, processing) = parse(text);
        let recipes = Recipes::new(crafting, processing);
        assert_eq!(
            recipes.process("test:cooking", ItemType::RawIron),
            Some(ItemStack::new(ItemType::IronIngot, 1))
        );
    }

    #[test]
    fn a_pack_patch_retires_a_recipe_it_does_not_own() {
        let base = r#"{ "recipes": [{
            "type":"crafting","recipe":"petramond:test_retire","station":"petramond:inventory",
            "ingredients":[{"item":"petramond:iron_ingot","count":3}],
            "result":{"item":"petramond:iron_pickaxe","count":1}
        }] }"#;
        let layers = |pack: Option<&str>| {
            let mut layers = vec![layer(base, "<base>", None, &[])];
            if let Some(text) = pack {
                layers.push(layer(text, "forge/recipes.json", Some("forge"), &["forge"]));
            }
            layers
        };
        let loaded = |pack: Option<&str>| load_layers(layers(pack), &Default::default());

        assert!(loaded(None)
            .crafting()
            .get("petramond:test_retire")
            .is_some());

        // A pack cannot restate an engine recipe key, but it can PATCH one —
        // which is how it replaces a whole crafting route with its own.
        let retire = r#"{ "recipes": [
            {"patch":"petramond:test_retire","data":{"petramond:enabled":false}}
        ] }"#;
        assert!(loaded(Some(retire))
            .crafting()
            .get("petramond:test_retire")
            .is_none());

        // A PROCESSING row is retired by the same key — the forge takes raw
        // metal off the ordinary furnace this way.
        let smelt = r#"{ "recipes": [{
            "type":"processing","recipe":"petramond:test_smelt","class":"petramond:smelting",
            "ingredient":"petramond:raw_iron","result":"petramond:iron_ingot"
        }] }"#;
        let retire_smelt = r#"{ "recipes": [
            {"patch":"petramond:test_smelt","data":{"petramond:enabled":false}}
        ] }"#;
        let with_smelt = |pack: Option<&str>| {
            let mut layers = vec![layer(smelt, "<base>", None, &[])];
            if let Some(text) = pack {
                layers.push(layer(text, "forge/recipes.json", Some("forge"), &["forge"]));
            }
            load_layers(layers, &Default::default())
        };
        assert!(with_smelt(None)
            .process("petramond:smelting", ItemType::RawIron)
            .is_some());
        assert!(with_smelt(Some(retire_smelt))
            .process("petramond:smelting", ItemType::RawIron)
            .is_none());

        // A non-boolean value is a load error, not a silent "still enabled".
        let malformed = r#"{ "recipes": [
            {"patch":"petramond:test_retire","data":{"petramond:enabled":"no"}}
        ] }"#;
        assert!(loaded(Some(malformed))
            .crafting()
            .get("petramond:test_retire")
            .is_none());
    }

    #[test]
    fn disabled_references_and_disabled_layer_owners_are_removed() {
        let mut disabled = std::collections::BTreeSet::new();
        disabled.insert("wheel".to_owned());
        let cross_ref = r#"{ "recipes": [{
            "type":"crafting","recipe":"petramond:test","station":"petramond:inventory",
            "ingredients":[{"item":"wheel:wheel_of_fortune","count":1}],
            "result":{"item":"petramond:stick","count":1}
        }] }"#;
        assert!(parse_for(cross_ref, &disabled, None, &mut Vec::new())
            .0
            .is_empty());

        // This row mentions only engine content, so reference filtering alone
        // cannot remove it. Disabling its owning pack must remove the layer
        // before parse, including non-selectable processing rows.
        let core_only = r#"{ "recipes": [{
            "type":"processing","recipe":"wheel:test_disabled_owner",
            "class":"petramond:test_disabled_owner",
            "ingredient":"petramond:coal","result":"petramond:stick"
        }] }"#;
        let layers = || {
            vec![layer(
                core_only,
                "wheel/recipes.json",
                Some("wheel"),
                &["wheel"],
            )]
        };
        assert_eq!(
            load_layers(layers(), &Default::default())
                .process("petramond:test_disabled_owner", ItemType::Coal),
            Some(ItemStack::new(ItemType::Stick, 1))
        );
        assert_eq!(
            load_layers(layers(), &disabled)
                .process("petramond:test_disabled_owner", ItemType::Coal),
            None
        );
    }
}
