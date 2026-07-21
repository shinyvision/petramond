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

const EMBEDDED: &str = include_str!("../../assets/recipes.json");

#[derive(Deserialize)]
struct RawFile {
    recipes: Vec<RawRecipe>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawRecipe {
    Crafting {
        recipe: String,
        station: String,
        ingredients: Vec<RawCraftingIngredient>,
        result: RawStack,
    },
    Processing {
        class: String,
        ingredient: String,
        result: String,
        #[serde(default = "one_u8")]
        count: u8,
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
    Crafting(CraftingRecipe),
    Processing(ProcessingRecipe),
}

fn one_u8() -> u8 {
    1
}

#[cfg(test)]
pub fn load_recipes() -> Recipes {
    load_recipes_for(&std::collections::BTreeSet::new())
}

/// Load base + enabled pack recipe layers in deterministic pack order.
///
/// Layer ownership is retained: disabling a pack removes its whole layer even
/// when one of its rows mentions engine content only. Reference filtering then
/// removes enabled/base rows that touch another disabled namespace.
pub fn load_recipes_for(disabled: &std::collections::BTreeSet<String>) -> Recipes {
    load_layers(read_recipe_layers(), disabled)
}

fn load_layers(
    layers: impl IntoIterator<Item = (String, std::path::PathBuf, Option<String>)>,
    disabled: &std::collections::BTreeSet<String>,
) -> Recipes {
    let mut crafting = Vec::new();
    let mut processing = Vec::new();
    for (text, path, owner) in layers {
        if owner.as_ref().is_some_and(|id| disabled.contains(id)) {
            log::info!(
                "skipping recipes layer {}: owning mod is disabled",
                path.display()
            );
            continue;
        }
        log::info!("crafting recipes layer: {}", path.display());
        let (c, p) = parse_for(&text, disabled, owner.as_deref());
        crafting.extend(c);
        processing.extend(p);
    }
    Recipes::new(crafting, processing)
}

fn read_recipe_layers() -> Vec<(String, std::path::PathBuf, Option<String>)> {
    let layers = crate::assets::read_layers_with_ids("recipes.json");
    if layers.is_empty() {
        log::info!("crafting recipes: no on-disk recipes.json found, using embedded defaults");
        vec![(
            EMBEDDED.to_owned(),
            std::path::PathBuf::from("<embedded recipes.json>"),
            None,
        )]
    } else {
        layers
    }
}

#[cfg(test)]
fn parse(text: &str) -> (Vec<CraftingRecipe>, Vec<ProcessingRecipe>) {
    parse_for(text, &std::collections::BTreeSet::new(), None)
}

fn parse_for(
    text: &str,
    disabled: &std::collections::BTreeSet<String>,
    owner: Option<&str>,
) -> (Vec<CraftingRecipe>, Vec<ProcessingRecipe>) {
    let file: RawFile = match serde_json::from_str(text) {
        Ok(file) => file,
        Err(error) => {
            log::error!("recipes.json is not valid JSON: {error}");
            return (Vec::new(), Vec::new());
        }
    };
    let mut crafting = Vec::new();
    let mut processing = Vec::new();
    for (index, raw) in file.recipes.into_iter().enumerate() {
        if let Some(namespace) = disabled_namespace_in(&raw, disabled) {
            log::info!("skipping recipe #{index}: it references disabled mod '{namespace}'");
            continue;
        }
        match convert(raw, owner) {
            Ok(Converted::Crafting(recipe)) => crafting.push(recipe),
            Ok(Converted::Processing(recipe)) => processing.push(recipe),
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
        } => {
            validate_recipe_owner(&recipe, owner)?;
            let station = CraftingStation::from_key(&station)
                .ok_or_else(|| format!("unknown crafting station '{station}'"))?;
            let ingredients: Vec<CraftingIngredient> = ingredients
                .into_iter()
                .map(convert_ingredient)
                .collect::<Result<_, _>>()?;
            let result_item = resolve_item(&result.item)?;
            Ok(Converted::Crafting(CraftingRecipe::try_new(
                recipe,
                station,
                ingredients,
                ItemStack {
                    item: result_item,
                    count: result.count,
                },
            )?))
        }
        RawRecipe::Processing {
            class,
            ingredient,
            result,
            count,
        } => {
            if !crate::registry::is_namespaced(&class) {
                return Err(format!("processing class '{class}' is not namespaced"));
            }
            let input = resolve_item(&ingredient)?;
            let result = resolve_item(&result)?;
            validate_stack_count(result, count, "processing result")?;
            Ok(Converted::Processing(ProcessingRecipe {
                class,
                input,
                result: ItemStack::new(result, count),
            }))
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

fn validate_recipe_owner(key: &str, owner: Option<&str>) -> Result<(), String> {
    let namespace = crate::registry::namespace(key)
        .ok_or_else(|| format!("crafting recipe key '{key}' is not namespaced"))?;
    match owner {
        Some(owner) if namespace == owner => Ok(()),
        Some(owner) => Err(format!(
            "crafting recipe key '{key}' does not belong to pack '{owner}'"
        )),
        None if namespace == crate::registry::ENGINE_NAMESPACE => Ok(()),
        None => Err(format!(
            "crafting recipe key '{key}' ships without its owning pack"
        )),
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
            class,
            ingredient,
            result,
            ..
        } => hit(class)
            .or_else(|| hit(ingredient))
            .or_else(|| hit(result)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
             "ingredients":[{"item":"petramond:wooden_shovel","count":1,"keep":true}],
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
            {"type":"processing","class":"test:cooking","ingredient":"petramond:raw_iron","result":"petramond:iron_ingot"}
        ] }"#;
        let (crafting, processing) = parse(text);
        let recipes = Recipes::new(crafting, processing);
        assert_eq!(
            recipes.process("test:cooking", ItemType::RawIron),
            Some(ItemStack::new(ItemType::IronIngot, 1))
        );
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
        assert!(parse_for(cross_ref, &disabled, None).0.is_empty());

        // This row mentions only engine content, so reference filtering alone
        // cannot remove it. Disabling its owning pack must remove the layer
        // before parse, including non-selectable processing rows.
        let core_only = r#"{ "recipes": [{
            "type":"processing","class":"petramond:test_disabled_owner",
            "ingredient":"petramond:coal","result":"petramond:stick"
        }] }"#;
        let layer = || {
            vec![(
                core_only.to_owned(),
                std::path::PathBuf::from("wheel/recipes.json"),
                Some("wheel".to_owned()),
            )]
        };
        assert_eq!(
            load_layers(layer(), &Default::default())
                .process("petramond:test_disabled_owner", ItemType::Coal),
            Some(ItemStack::new(ItemType::Stick, 1))
        );
        assert_eq!(
            load_layers(layer(), &disabled)
                .process("petramond:test_disabled_owner", ItemType::Coal),
            None
        );
    }

    /// A pack-defined recipe reaches the same catalog/planner as engine data;
    /// its tag selector accepts an engine item without a WASM registration API.
    #[test]
    fn pack_crafting_recipe_uses_the_engine_planner() {
        let Some(root) = crate::modding::tests::stage_mods_fixture("boat-recipe", &["vehicles"])
        else {
            return;
        };
        crate::modding::tests::run_child_test(&root, "crafting::load::tests::boat_recipe_inner");
    }

    #[test]
    #[ignore = "spawned by pack_crafting_recipe_uses_the_engine_planner with a fixture pack env"]
    fn boat_recipe_inner() {
        let recipes = load_recipes();
        let recipe = recipes
            .crafting()
            .get("vehicles:boat")
            .expect("pack crafting recipe loaded");
        let mut inventory = crate::inventory::Inventory::new();
        inventory.add(ItemStack::new(ItemType::BirchPlanks, 5));
        assert!(recipe.craftable_with(&inventory));
    }
}
