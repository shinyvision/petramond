//! Load recipes from `assets/recipes.json` (serde).
//!
//! The on-disk file is preferred so recipes can be edited without a rebuild; an
//! embedded copy is the fallback when the game runs outside the project tree.
//! Items are referenced by their stable snake_case [`key`](ItemType::key) (e.g.
//! `oak_planks`) — the item's real identity, independent of its display name; a
//! `#name` reference is a tag.

use std::collections::HashMap;

use serde::Deserialize;

use crate::item::{ItemStack, ItemTag, ItemType};

use super::recipe::{FurnitureRecipe, Ingredient, ProcessingRecipe, Recipe, Recipes};

/// Embedded fallback, so the game always has recipes even when run outside the
/// project tree. The on-disk copy, when found, takes priority.
const EMBEDDED: &str = include_str!("../../assets/recipes.json");

#[derive(Deserialize)]
struct RawFile {
    recipes: Vec<RawRecipe>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RawRecipe {
    Shapeless {
        ingredients: Vec<String>,
        result: String,
        #[serde(default = "one")]
        count: u8,
    },
    Shaped {
        pattern: Vec<String>,
        key: HashMap<String, String>,
        result: String,
        #[serde(default = "one")]
        count: u8,
    },
    /// A machine-processing recipe: one `ingredient` item produces `result`
    /// in machines consuming the namespaced `class` (`petramond:smelting` = the
    /// furnace; a mod machine names its own, e.g. `kitchen:cooking`). Any pack
    /// may target any class — that composition is the point.
    Processing {
        class: String,
        ingredient: String,
        result: String,
        #[serde(default = "one")]
        count: u8,
    },
    /// A furniture-workbench recipe: placing `cost` of `input` lets you craft `count`
    /// of `result`. The workbench offers it whenever that block is in the input slot.
    Furniture {
        input: String,
        result: String,
        /// Input items consumed per craft (default 1).
        #[serde(default = "one")]
        cost: u8,
        /// Result items produced per craft (default 1).
        #[serde(default = "one")]
        count: u8,
    },
}

/// A converted recipe sorted into the grid list, the processing table, or the
/// furniture-workbench table.
enum Converted {
    Grid(Recipe),
    Processing(ProcessingRecipe),
    Furniture(FurnitureRecipe),
}

fn one() -> u8 {
    1
}

/// [`load_recipes_for`] with nothing disabled — the test convenience.
#[cfg(test)]
pub fn load_recipes() -> Recipes {
    load_recipes_for(&std::collections::BTreeSet::new())
}

/// Load the recipe set from every `recipes.json` layer (base + mod packs —
/// recipes have no identity key, so pack layers APPEND recipes), falling back
/// to the embedded copy when nothing on disk provides one. Malformed
/// individual recipes are logged and skipped rather than aborting the world
/// load.
///
/// Recipes whose result or ingredients reference content namespaced to a mod
/// id in `disabled` (per-world `settings.json`) are dropped: a disabled mod's
/// items must not be craftable INTO or FROM. Filtering is by the raw key
/// strings, before item resolution — the items themselves stay registered
/// process-wide.
pub fn load_recipes_for(disabled: &std::collections::BTreeSet<String>) -> Recipes {
    let mut grid = Vec::new();
    let mut processing = Vec::new();
    let mut furniture = Vec::new();
    for text in read_recipes_layers() {
        let (g, s, f) = parse_for(&text, disabled);
        grid.extend(g);
        processing.extend(s);
        furniture.extend(f);
    }
    Recipes::new(grid, processing, furniture)
}

fn read_recipes_layers() -> Vec<String> {
    let layers = crate::assets::read_layers("recipes.json");
    if layers.is_empty() {
        log::info!("crafting recipes: no on-disk recipes.json found, using embedded defaults");
        return vec![EMBEDDED.to_string()];
    }
    for (_, path) in &layers {
        log::info!("crafting recipes layer: {}", path.display());
    }
    layers.into_iter().map(|(s, _)| s).collect()
}

#[cfg(test)]
fn parse(text: &str) -> (Vec<Recipe>, Vec<ProcessingRecipe>, Vec<FurnitureRecipe>) {
    parse_for(text, &std::collections::BTreeSet::new())
}

fn parse_for(
    text: &str,
    disabled: &std::collections::BTreeSet<String>,
) -> (Vec<Recipe>, Vec<ProcessingRecipe>, Vec<FurnitureRecipe>) {
    let file: RawFile = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            log::error!("recipes.json is not valid JSON: {e}");
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };
    let mut grid = Vec::new();
    let mut processing = Vec::new();
    let mut furniture = Vec::new();
    for (i, raw) in file.recipes.into_iter().enumerate() {
        if let Some(ns) = disabled_namespace_in(&raw, disabled) {
            log::info!("skipping recipe #{i}: it references content of the disabled mod '{ns}'");
            continue;
        }
        match convert(raw) {
            Ok(Converted::Grid(r)) => grid.push(r),
            Ok(Converted::Processing(r)) => processing.push(r),
            Ok(Converted::Furniture(r)) => furniture.push(r),
            Err(e) => log::error!("skipping recipe #{i}: {e}"),
        }
    }
    (grid, processing, furniture)
}

/// The first disabled mod id `raw`'s result or ingredient keys reference, or
/// `None` when the recipe is clean. `#tag` references check the tag key the
/// same way (engine tags are bare, so they never match).
fn disabled_namespace_in<'a>(
    raw: &RawRecipe,
    disabled: &'a std::collections::BTreeSet<String>,
) -> Option<&'a str> {
    let hit = |s: &str| -> Option<&'a str> {
        let key = s.strip_prefix('#').unwrap_or(s);
        let ns = crate::registry::namespace(key)?;
        disabled.get(ns).map(String::as_str)
    };
    match raw {
        RawRecipe::Shapeless {
            ingredients,
            result,
            ..
        } => ingredients
            .iter()
            .find_map(|s| hit(s))
            .or_else(|| hit(result)),
        RawRecipe::Shaped { key, result, .. } => {
            key.values().find_map(|s| hit(s)).or_else(|| hit(result))
        }
        RawRecipe::Processing {
            class,
            ingredient,
            result,
            ..
        } => hit(class)
            .or_else(|| hit(ingredient))
            .or_else(|| hit(result)),
        RawRecipe::Furniture { input, result, .. } => hit(input).or_else(|| hit(result)),
    }
}

fn convert(raw: RawRecipe) -> Result<Converted, String> {
    match raw {
        RawRecipe::Processing {
            class,
            ingredient,
            result,
            count,
        } => {
            // Class keys are public machine selectors — namespaced like every
            // other public key (a bare typo must not mint a machine class).
            if !crate::registry::is_namespaced(&class) {
                return Err(format!(
                    "processing class '{class}' must be namespaced ('mod_id:name')"
                ));
            }
            let input = item_from_key(&ingredient)
                .ok_or_else(|| format!("unknown processing ingredient '{ingredient}'"))?;
            let result = item_stack(&result, count)?;
            Ok(Converted::Processing(ProcessingRecipe {
                class,
                input,
                result,
            }))
        }
        RawRecipe::Furniture {
            input,
            result,
            cost,
            count,
        } => {
            let input = item_from_key(&input)
                .ok_or_else(|| format!("unknown furniture input '{input}'"))?;
            let result = item_stack(&result, count)?;
            Ok(Converted::Furniture(FurnitureRecipe {
                input,
                result,
                cost: cost.max(1),
            }))
        }
        RawRecipe::Shapeless {
            ingredients,
            result,
            count,
        } => {
            let result = item_stack(&result, count)?;
            let ingredients = ingredients
                .iter()
                .map(|s| parse_ingredient(s))
                .collect::<Result<Vec<_>, _>>()?;
            if ingredients.is_empty() {
                return Err("shapeless recipe has no ingredients".into());
            }
            if ingredients.len() > super::MAX_CELLS {
                return Err(format!(
                    "shapeless recipe has {} ingredients (max {})",
                    ingredients.len(),
                    super::MAX_CELLS
                ));
            }
            Ok(Converted::Grid(Recipe::Shapeless {
                ingredients,
                result,
            }))
        }
        RawRecipe::Shaped {
            pattern,
            key,
            result,
            count,
        } => {
            let result = item_stack(&result, count)?;
            if pattern.is_empty() {
                return Err("shaped recipe has an empty pattern".into());
            }
            let height = pattern.len();
            let width = pattern.iter().map(|r| r.chars().count()).max().unwrap_or(0);
            if width == 0 {
                return Err("shaped recipe pattern has zero width".into());
            }
            if width > super::MAX_GRID || height > super::MAX_GRID {
                return Err(format!(
                    "shaped recipe is {width}x{height}, exceeds {0}x{0}",
                    super::MAX_GRID
                ));
            }
            let mut cells = Vec::with_capacity(width * height);
            for row in &pattern {
                let chars: Vec<char> = row.chars().collect();
                for c in 0..width {
                    match chars.get(c).copied().unwrap_or(' ') {
                        ' ' => cells.push(None),
                        ch => {
                            let sym = key
                                .get(&ch.to_string())
                                .ok_or_else(|| format!("pattern char '{ch}' missing from key"))?;
                            cells.push(Some(parse_ingredient(sym)?));
                        }
                    }
                }
            }
            Ok(Converted::Grid(Recipe::Shaped {
                width,
                height,
                cells,
                result,
            }))
        }
    }
}

/// Parse one ingredient string: `#tag` → an item tag (resolved against item
/// data), otherwise an exact item key.
fn parse_ingredient(s: &str) -> Result<Ingredient, String> {
    if let Some(tag) = s.strip_prefix('#') {
        ItemTag::from_key(tag)
            .map(Ingredient::Tag)
            .ok_or_else(|| format!("unknown tag '#{tag}'"))
    } else {
        item_from_key(s)
            .map(Ingredient::Item)
            .ok_or_else(|| format!("unknown item '{s}'"))
    }
}

fn item_stack(key: &str, count: u8) -> Result<ItemStack, String> {
    let item = item_from_key(key).ok_or_else(|| format!("unknown result item '{key}'"))?;
    Ok(ItemStack::new(item, count.max(1)))
}

/// Resolve a recipe's snake_case [`key`](ItemType::key) (e.g. `oak_planks`) to its
/// item — matched against each item's explicit stable key, not its display name.
fn item_from_key(key: &str) -> Option<ItemType> {
    ItemType::all().iter().copied().find(|it| it.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_recipes_parse_without_error() {
        // The shipped recipes.json must convert fully (no skipped recipes).
        let (grid, processing, furniture) = parse(EMBEDDED);
        let raw: RawFile = serde_json::from_str(EMBEDDED).expect("valid json");
        assert_eq!(
            grid.len() + processing.len() + furniture.len(),
            raw.recipes.len(),
            "every shipped recipe should convert"
        );
        // Sanity: at least the 8 plank recipes + table + sticks + 2 pickaxes + furnace.
        assert!(grid.len() >= 13);
        // Iron + copper smelting at minimum.
        assert!(processing.len() >= 2);
        // The 8 plank → door furniture recipes.
        assert!(furniture.len() >= 8);
    }

    #[test]
    fn processing_recipes_parse_by_class_and_skip_bad_rows() {
        let text = r#"{ "recipes": [
            { "type": "processing", "class": "petramond:smelting", "ingredient": "petramond:raw_iron", "result": "petramond:iron_ingot" },
            { "type": "processing", "class": "kitchen:cooking", "ingredient": "petramond:raw_iron", "result": "petramond:coal" },
            { "type": "processing", "class": "petramond:smelting", "ingredient": "mystery", "result": "petramond:iron_ingot" },
            { "type": "processing", "class": "bareclass", "ingredient": "petramond:raw_iron", "result": "petramond:iron_ingot" }
        ] }"#;
        let (grid, processing, _furniture) = parse(text);
        assert!(grid.is_empty());
        assert_eq!(
            processing.len(),
            2,
            "unknown-ingredient and bare-class rows are skipped"
        );
        // Same input, different machines, different products: the class is
        // the lookup key that keeps an oven from smelting ore.
        let recipes = Recipes::new(Vec::new(), processing, Vec::new());
        assert_eq!(
            recipes.process("petramond:smelting", ItemType::RawIron),
            Some(ItemStack::new(ItemType::IronIngot, 1))
        );
        assert_eq!(
            recipes.smelt(ItemType::RawIron).unwrap().item,
            ItemType::IronIngot
        );
        assert_eq!(
            recipes.process("kitchen:cooking", ItemType::RawIron),
            Some(ItemStack::new(ItemType::Coal, 1))
        );
        assert_eq!(recipes.process("other:class", ItemType::RawIron), None);
    }

    #[test]
    fn furniture_recipes_parse_and_look_up_by_input() {
        let text = r#"{ "recipes": [
            { "type": "furniture", "input": "petramond:oak_planks", "result": "petramond:oak_door", "cost": 1 },
            { "type": "furniture", "input": "petramond:spruce_planks", "result": "petramond:spruce_door", "cost": 6 },
            { "type": "furniture", "input": "mystery", "result": "petramond:oak_door" }
        ] }"#;
        let (grid, processing, furniture) = parse(text);
        assert!(grid.is_empty() && processing.is_empty());
        assert_eq!(furniture.len(), 2, "the unknown-input recipe is skipped");
        let recipes = Recipes::new(grid, processing, furniture);
        // The workbench offers oak_door for oak_planks (cost 1) and nothing for a log.
        let oak: Vec<_> = recipes.furniture_for(ItemType::OakPlanks).collect();
        assert_eq!(oak.len(), 1);
        assert_eq!(oak[0].result.item, ItemType::OakDoor);
        assert_eq!(oak[0].cost, 1);
        let spruce: Vec<_> = recipes.furniture_for(ItemType::SprucePlanks).collect();
        assert_eq!(spruce[0].cost, 6, "cost carries through");
        assert_eq!(recipes.furniture_for(ItemType::OakLog).count(), 0);
    }

    #[test]
    fn tag_and_item_ingredients_parse() {
        assert!(matches!(
            parse_ingredient("#petramond:planks"),
            Ok(Ingredient::Tag(_))
        ));
        assert!(matches!(
            parse_ingredient("petramond:stick"),
            Ok(Ingredient::Item(ItemType::Stick))
        ));
        assert!(parse_ingredient("#bogus").is_err());
        assert!(parse_ingredient("bogus_item").is_err());
    }

    /// A mod pack's shaped recipe (stick plus around a `#petramond:logs` centre → the
    /// pack's own dynamic item) resolves through the ENGINE recipe matcher:
    /// pack recipes.json layers append, `#tag` ingredients resolve, and the
    /// namespaced result key finds the dynamically registered item. Pack
    /// registration needs the fixture in the registry, so the assertions run
    /// in a child process (the established `PETRAMOND_MODS` re-spawn
    /// pattern, staged by `modding::tests`).
    #[test]
    fn wheel_mod_shaped_recipe_resolves_through_the_engine_matcher() {
        let Some(root) = crate::modding::tests::stage_mods_fixture("wheel-recipe", &["wheel"])
        else {
            return;
        };
        crate::modding::tests::run_child_test(&root, "crafting::load::tests::wheel_recipe_inner");
    }

    /// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
    /// pointing at the fixture pack before first registry touch).
    #[test]
    #[ignore = "spawned by wheel_mod_shaped_recipe_resolves_through_the_engine_matcher with a fixture pack env"]
    fn wheel_recipe_inner() {
        let wheel = item_from_key("wheel:wheel_of_fortune")
            .expect("wheel item registered from the fixture pack");
        let recipes = load_recipes();

        let grid = |cells: [Option<ItemType>; 9]| -> Vec<Option<ItemStack>> {
            cells
                .iter()
                .map(|o| o.map(|i| ItemStack::new(i, 1)))
                .collect()
        };
        let (s, log) = (Some(ItemType::Stick), Some(ItemType::BirchLog));

        // The plus arrangement: sticks NESW around any `#petramond:logs` centre.
        let plus = grid([None, s, None, s, log, s, None, s, None]);
        assert_eq!(
            recipes.find(&plus, 3),
            Some(ItemStack::new(wheel, 1)),
            "stick plus around a log crafts the wheel"
        );
        // A wrong arrangement (diagonal sticks) matches nothing.
        let x_shape = grid([s, None, s, None, log, None, s, None, s]);
        assert_eq!(
            recipes.find(&x_shape, 3),
            None,
            "the X arrangement is not the wheel recipe"
        );

        // Per-world disable: with the wheel mod disabled, the same session's
        // recipe build drops the recipe even though the item stays registered.
        let disabled: std::collections::BTreeSet<String> = ["wheel".to_owned()].into();
        let filtered = load_recipes_for(&disabled);
        assert_eq!(
            filtered.find(&plus, 3),
            None,
            "a disabled mod's recipe is not offered"
        );
        assert!(
            filtered.len() < recipes.len(),
            "only the wheel recipe was dropped"
        );
    }

    /// Per-world disabled mods: a recipe is dropped when its RESULT, any
    /// INGREDIENT (shaped key, shapeless list, processing input, furniture
    /// input — `#tag` keys included), or its processing CLASS is namespaced to
    /// a disabled mod id; engine `petramond:*` keys and other namespaces pass.
    #[test]
    fn recipes_touching_a_disabled_namespace_are_filtered() {
        let disabled: std::collections::BTreeSet<String> = ["wheel".to_owned()].into();
        let raw = |json: &str| serde_json::from_str::<RawRecipe>(json).expect("recipe json");

        let hits = [
            r##"{ "type": "shaped", "pattern": ["L"], "key": {"L": "#petramond:logs"}, "result": "wheel:wheel_of_fortune" }"##,
            r##"{ "type": "shapeless", "ingredients": ["wheel:wheel_of_fortune"], "result": "petramond:stick" }"##,
            r##"{ "type": "shaped", "pattern": ["W"], "key": {"W": "wheel:wheel_of_fortune"}, "result": "petramond:stick" }"##,
            r##"{ "type": "shapeless", "ingredients": ["#wheel:parts"], "result": "petramond:stick" }"##,
            r##"{ "type": "processing", "class": "petramond:smelting", "ingredient": "wheel:wheel_of_fortune", "result": "petramond:coal" }"##,
            r##"{ "type": "processing", "class": "wheel:spinning", "ingredient": "petramond:coal", "result": "petramond:stick" }"##,
            r##"{ "type": "furniture", "input": "petramond:oak_planks", "result": "wheel:wheel_of_fortune" }"##,
        ];
        for json in hits {
            assert_eq!(
                disabled_namespace_in(&raw(json), &disabled),
                Some("wheel"),
                "{json}"
            );
        }

        let passes = [
            r#"{ "type": "shapeless", "ingredients": ["petramond:oak_log"], "result": "petramond:oak_planks" }"#,
            r#"{ "type": "shapeless", "ingredients": ["othermod:gear"], "result": "petramond:stick" }"#,
        ];
        for json in passes {
            assert_eq!(disabled_namespace_in(&raw(json), &disabled), None, "{json}");
        }
    }

    #[test]
    fn bad_recipes_are_skipped_not_fatal() {
        let text = r#"{ "recipes": [
            { "type": "shapeless", "ingredients": ["petramond:oak_log"], "result": "petramond:oak_planks", "count": 4 },
            { "type": "shapeless", "ingredients": ["mystery"], "result": "petramond:oak_planks" },
            { "type": "shaped", "pattern": ["X"], "key": {}, "result": "petramond:stick" }
        ] }"#;
        // Only the first (valid) recipe survives; the other two are skipped.
        let (grid, processing, furniture) = parse(text);
        assert_eq!(grid.len(), 1);
        assert!(processing.is_empty());
        assert!(furniture.is_empty());
    }
}
