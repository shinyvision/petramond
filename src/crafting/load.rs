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

use super::recipe::{FurnitureRecipe, Ingredient, Recipe, Recipes, SmeltingRecipe};

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
    /// A furnace smelt: one `ingredient` item produces `result`.
    Smelting {
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

/// A converted recipe sorted into the grid list, the smelting table, or the
/// furniture-workbench table.
enum Converted {
    Grid(Recipe),
    Smelt(SmeltingRecipe),
    Furniture(FurnitureRecipe),
}

fn one() -> u8 {
    1
}

/// Load the recipe set from every `recipes.json` layer (base + mod packs —
/// recipes have no identity key, so pack layers APPEND recipes), falling back
/// to the embedded copy when nothing on disk provides one. Malformed
/// individual recipes are logged and skipped rather than aborting the world
/// load.
pub fn load_recipes() -> Recipes {
    let mut grid = Vec::new();
    let mut smelting = Vec::new();
    let mut furniture = Vec::new();
    for text in read_recipes_layers() {
        let (g, s, f) = parse(&text);
        grid.extend(g);
        smelting.extend(s);
        furniture.extend(f);
    }
    Recipes::new(grid, smelting, furniture)
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

fn parse(text: &str) -> (Vec<Recipe>, Vec<SmeltingRecipe>, Vec<FurnitureRecipe>) {
    let file: RawFile = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            log::error!("recipes.json is not valid JSON: {e}");
            return (Vec::new(), Vec::new(), Vec::new());
        }
    };
    let mut grid = Vec::new();
    let mut smelting = Vec::new();
    let mut furniture = Vec::new();
    for (i, raw) in file.recipes.into_iter().enumerate() {
        match convert(raw) {
            Ok(Converted::Grid(r)) => grid.push(r),
            Ok(Converted::Smelt(r)) => smelting.push(r),
            Ok(Converted::Furniture(r)) => furniture.push(r),
            Err(e) => log::error!("skipping recipe #{i}: {e}"),
        }
    }
    (grid, smelting, furniture)
}

fn convert(raw: RawRecipe) -> Result<Converted, String> {
    match raw {
        RawRecipe::Smelting {
            ingredient,
            result,
            count,
        } => {
            let input = item_from_key(&ingredient)
                .ok_or_else(|| format!("unknown smelting ingredient '{ingredient}'"))?;
            let result = item_stack(&result, count)?;
            Ok(Converted::Smelt(SmeltingRecipe { input, result }))
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
    ItemType::ALL.iter().copied().find(|it| it.key() == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_recipes_parse_without_error() {
        // The shipped recipes.json must convert fully (no skipped recipes).
        let (grid, smelting, furniture) = parse(EMBEDDED);
        let raw: RawFile = serde_json::from_str(EMBEDDED).expect("valid json");
        assert_eq!(
            grid.len() + smelting.len() + furniture.len(),
            raw.recipes.len(),
            "every shipped recipe should convert"
        );
        // Sanity: at least the 8 plank recipes + table + sticks + 2 pickaxes + furnace.
        assert!(grid.len() >= 13);
        // Iron + copper smelting at minimum.
        assert!(smelting.len() >= 2);
        // The 8 plank → door furniture recipes.
        assert!(furniture.len() >= 8);
    }

    #[test]
    fn smelting_recipes_parse_and_skip_unknown() {
        let text = r#"{ "recipes": [
            { "type": "smelting", "ingredient": "raw_iron", "result": "iron_ingot" },
            { "type": "smelting", "ingredient": "mystery", "result": "iron_ingot" }
        ] }"#;
        let (grid, smelting, _furniture) = parse(text);
        assert!(grid.is_empty());
        assert_eq!(
            smelting.len(),
            1,
            "the unknown-ingredient recipe is skipped"
        );
        assert_eq!(smelting[0].input, ItemType::RawIron);
        assert_eq!(smelting[0].result, ItemStack::new(ItemType::IronIngot, 1));
    }

    #[test]
    fn furniture_recipes_parse_and_look_up_by_input() {
        let text = r#"{ "recipes": [
            { "type": "furniture", "input": "oak_planks", "result": "oak_door", "cost": 1 },
            { "type": "furniture", "input": "spruce_planks", "result": "spruce_door", "cost": 6 },
            { "type": "furniture", "input": "mystery", "result": "oak_door" }
        ] }"#;
        let (grid, smelting, furniture) = parse(text);
        assert!(grid.is_empty() && smelting.is_empty());
        assert_eq!(furniture.len(), 2, "the unknown-input recipe is skipped");
        let recipes = Recipes::new(grid, smelting, furniture);
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
            parse_ingredient("#planks"),
            Ok(Ingredient::Tag(_))
        ));
        assert!(matches!(
            parse_ingredient("stick"),
            Ok(Ingredient::Item(ItemType::Stick))
        ));
        assert!(parse_ingredient("#bogus").is_err());
        assert!(parse_ingredient("bogus_item").is_err());
    }

    #[test]
    fn bad_recipes_are_skipped_not_fatal() {
        let text = r#"{ "recipes": [
            { "type": "shapeless", "ingredients": ["oak_log"], "result": "oak_planks", "count": 4 },
            { "type": "shapeless", "ingredients": ["mystery"], "result": "oak_planks" },
            { "type": "shaped", "pattern": ["X"], "key": {}, "result": "stick" }
        ] }"#;
        // Only the first (valid) recipe survives; the other two are skipped.
        let (grid, smelting, furniture) = parse(text);
        assert_eq!(grid.len(), 1);
        assert!(smelting.is_empty());
        assert!(furniture.is_empty());
    }
}
