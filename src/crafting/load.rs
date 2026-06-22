//! Load recipes from `assets/recipes.json` (serde).
//!
//! The on-disk file is preferred so recipes can be edited without a rebuild; an
//! embedded copy is the fallback when the game runs outside the project tree.
//! Items are referenced by their snake_case registry key (derived from the
//! display name, e.g. "Oak Planks" → `oak_planks`); a `#name` reference is a tag.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::item::{ItemStack, ItemTag, ItemType};

use super::recipe::{Ingredient, Recipe, Recipes};

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
}

fn one() -> u8 {
    1
}

/// Load the recipe set, preferring an on-disk `assets/recipes.json` and falling
/// back to the embedded copy. Malformed individual recipes are logged and
/// skipped rather than aborting the world load.
pub fn load_recipes() -> Recipes {
    Recipes::new(parse(&read_recipes_text()))
}

fn read_recipes_text() -> String {
    for path in candidate_paths() {
        if let Ok(s) = std::fs::read_to_string(&path) {
            log::info!("loaded crafting recipes from {}", path.display());
            return s;
        }
    }
    log::info!("crafting recipes: no on-disk recipes.json found, using embedded defaults");
    EMBEDDED.to_string()
}

/// Candidate locations, in priority order: an env override, the working-directory
/// `assets/`, then alongside the executable.
fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(dir) = std::env::var("LLAMACRAFT_ASSETS") {
        paths.push(PathBuf::from(dir).join("recipes.json"));
    }
    paths.push(PathBuf::from("assets/recipes.json"));
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("assets/recipes.json"));
            paths.push(dir.join("recipes.json"));
        }
    }
    paths
}

fn parse(text: &str) -> Vec<Recipe> {
    let file: RawFile = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            log::error!("recipes.json is not valid JSON: {e}");
            return Vec::new();
        }
    };
    let mut out = Vec::with_capacity(file.recipes.len());
    for (i, raw) in file.recipes.into_iter().enumerate() {
        match convert(raw) {
            Ok(r) => out.push(r),
            Err(e) => log::error!("skipping recipe #{i}: {e}"),
        }
    }
    out
}

fn convert(raw: RawRecipe) -> Result<Recipe, String> {
    match raw {
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
            Ok(Recipe::Shapeless {
                ingredients,
                result,
            })
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
            Ok(Recipe::Shaped {
                width,
                height,
                cells,
                result,
            })
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

/// Resolve a snake_case registry key (e.g. `oak_planks`) to its item, derived
/// from the item's display name ("Oak Planks" → `oak_planks`).
fn item_from_key(key: &str) -> Option<ItemType> {
    ItemType::ALL
        .iter()
        .copied()
        .find(|it| registry_key(*it) == key)
}

fn registry_key(item: ItemType) -> String {
    item.name().to_ascii_lowercase().replace(' ', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_recipes_parse_without_error() {
        // The shipped recipes.json must convert fully (no skipped recipes).
        let parsed = parse(EMBEDDED);
        let raw: RawFile = serde_json::from_str(EMBEDDED).expect("valid json");
        assert_eq!(
            parsed.len(),
            raw.recipes.len(),
            "every shipped recipe should convert"
        );
        // Sanity: at least the 8 plank recipes + table + sticks + 2 pickaxes.
        assert!(parsed.len() >= 12);
    }

    #[test]
    fn registry_keys_match_display_names() {
        assert_eq!(item_from_key("oak_log"), Some(ItemType::OakLog));
        assert_eq!(item_from_key("dark_oak_planks"), Some(ItemType::DarkOakPlanks));
        assert_eq!(item_from_key("crafting_table"), Some(ItemType::CraftingTable));
        assert_eq!(item_from_key("wooden_pickaxe"), Some(ItemType::WoodenPickaxe));
        assert_eq!(item_from_key("cobblestone"), Some(ItemType::Cobblestone));
        assert_eq!(item_from_key("nope"), None);
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
        assert_eq!(parse(text).len(), 1);
    }
}
