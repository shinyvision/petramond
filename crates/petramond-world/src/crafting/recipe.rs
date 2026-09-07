//! Resolved recipe data and catalog lookups.
//!
//! Player crafting is inventory-driven: recipes declare aggregate ingredient
//! quantities and a minimum station, never a grid arrangement. Processing
//! recipes remain separate because their interaction model differs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemTag, ItemType};

pub const MAX_INGREDIENT_UNITS: u32 = crate::inventory::TOTAL_SLOTS as u32 * u8::MAX as u32;

/// The furnace's processing-recipe class (see [`ProcessingRecipe::class`]).
pub const SMELTING_CLASS: &str = "petramond:smelting";

use super::station::CraftingStation;

/// An exact item or an open, item-owned tag selector.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IngredientSelector {
    Item(ItemType),
    Tag(ItemTag),
}

impl IngredientSelector {
    #[inline]
    pub fn matches(self, item: ItemType) -> bool {
        match self {
            Self::Item(exact) => exact == item,
            Self::Tag(tag) => item.has_tag(tag),
        }
    }

    /// A deterministic icon for the recipe browser. Prefer an owned matching
    /// item because that is what the planner can actually consume; otherwise
    /// use the first registered tag member as the unavailable-row exemplar.
    pub fn display_item(self, inventory: &Inventory) -> Option<ItemType> {
        match self {
            Self::Item(item) => Some(item),
            Self::Tag(tag) => inventory
                .raw_slots()
                .iter()
                .flatten()
                .find(|stack| stack.item.has_tag(tag))
                .map(|stack| stack.item)
                .or_else(|| {
                    ItemType::all()
                        .iter()
                        .copied()
                        .find(|item| item.has_tag(tag))
                }),
        }
    }
}

/// What one assigned ingredient unit does when CRAFT commits.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum IngredientUse {
    Consume,
    Keep,
    Remainder(ItemType),
}

/// One aggregate player-crafting ingredient row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingIngredient {
    pub selector: IngredientSelector,
    pub count: u16,
    pub use_mode: IngredientUse,
}

/// One selectable player-crafting recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CraftingRecipe {
    key: String,
    station: CraftingStation,
    ingredients: Vec<CraftingIngredient>,
    result: ItemStack,
    /// The row's namespaced consumer-data entries (`(key, canonical JSON)`),
    /// merged from the row's own `data` map and every patch row targeting it —
    /// the recipe interop surface, same shape as item/block row data.
    data: Vec<(String, String)>,
    /// Compiled `petramond:inherit` entry: per-stack instance-data keys the
    /// crafted output copies from the consumed ingredients. Ingredients that
    /// carry one of these keys must AGREE on its value or the recipe does not
    /// match (see `plan`).
    inherit: Vec<String>,
}

impl CraftingRecipe {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        key: String,
        station: CraftingStation,
        ingredients: Vec<CraftingIngredient>,
        result: ItemStack,
    ) -> Self {
        Self {
            key,
            station,
            ingredients,
            result,
            data: Vec::new(),
            inherit: Vec::new(),
        }
    }

    pub fn try_new(
        key: String,
        station: CraftingStation,
        ingredients: Vec<CraftingIngredient>,
        result: ItemStack,
    ) -> Result<Self, String> {
        if !crate::registry::is_namespaced(&key) {
            return Err(format!("crafting recipe key '{key}' is not namespaced"));
        }
        if ingredients.is_empty() {
            return Err("crafting recipe has no ingredients".into());
        }
        let total = ingredients.iter().try_fold(0u32, |total, ingredient| {
            total.checked_add(u32::from(ingredient.count))
        });
        let Some(total) = total else {
            return Err("crafting ingredient total overflows".into());
        };
        if total == 0 || total > MAX_INGREDIENT_UNITS {
            return Err(format!(
                "crafting recipe requires {total} units (allowed 1..={MAX_INGREDIENT_UNITS})"
            ));
        }
        for ingredient in &ingredients {
            if ingredient.count == 0 {
                return Err("crafting ingredient count is zero".into());
            }
            match ingredient.selector {
                IngredientSelector::Item(ItemType::Air) => {
                    return Err("crafting ingredient item is air".into())
                }
                IngredientSelector::Item(item) if item.max_stack_size() == 0 => {
                    return Err(format!(
                        "crafting ingredient item '{}' has zero stack size",
                        item.key()
                    ))
                }
                IngredientSelector::Tag(tag)
                    if !ItemType::all().iter().any(|item| item.has_tag(tag)) =>
                {
                    return Err(format!(
                        "crafting ingredient tag '{}' has no items",
                        public_tag_key(tag)
                    ))
                }
                _ => {}
            }
            if let IngredientUse::Remainder(item) = ingredient.use_mode {
                if item == ItemType::Air {
                    return Err("crafting remainder item is air".into());
                }
                if item.max_stack_size() == 0 {
                    return Err(format!(
                        "crafting remainder item '{}' has zero stack size",
                        item.key()
                    ));
                }
            }
        }
        if !ingredients
            .iter()
            .any(|ingredient| ingredient.use_mode != IngredientUse::Keep)
        {
            return Err("crafting recipe consumes no ingredient".into());
        }
        if result.item == ItemType::Air || result.count == 0 {
            return Err("crafting result is empty".into());
        }
        if result.count > result.item.max_stack_size() {
            return Err(format!(
                "result count {} does not fit one '{}' stack (max {})",
                result.count,
                result.item.key(),
                result.item.max_stack_size()
            ));
        }
        Ok(Self {
            key,
            station,
            ingredients,
            result,
            data: Vec::new(),
            inherit: Vec::new(),
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn station(&self) -> CraftingStation {
        self.station
    }

    pub fn ingredients(&self) -> &[CraftingIngredient] {
        &self.ingredients
    }

    pub fn result(&self) -> ItemStack {
        self.result
    }

    /// The row's consumer-data entry `key` as raw JSON text, or `None` — the
    /// recipe interop surface (`"data"` in `recipes.json` + patch rows).
    pub fn data_value(&self, key: &str) -> Option<&str> {
        self.data
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Instance-data keys the output inherits from consumed ingredients.
    pub fn inherit(&self) -> &[String] {
        &self.inherit
    }

    /// Whether the row joins the catalog — the engine's `petramond:enabled`
    /// vocabulary ([`crate::registry::row_enabled`]). This is how a pack
    /// RETIRES a recipe it does not own: it cannot restate
    /// `petramond:iron_pickaxe` (namespace ownership) but it can attach data
    /// to it with a patch row, so replacing a whole crafting route is the
    /// retirement patch plus the pack's own replacement rows.
    pub fn row_enabled(data: &[(String, String)]) -> Result<bool, String> {
        crate::registry::row_enabled(data)
    }

    /// Attach the row's compiled data entries, parsing the engine's
    /// `petramond:inherit` vocabulary strictly (a malformed entry fails the
    /// row, like fuel/tool on items).
    pub fn set_data(&mut self, data: Vec<(String, String)>) -> Result<(), String> {
        let inherit = match data.iter().find(|(k, _)| k == "petramond:inherit") {
            None => Vec::new(),
            Some((_, text)) => {
                let keys: Vec<String> = serde_json::from_str(text)
                    .map_err(|e| format!("malformed 'petramond:inherit' data: {e}"))?;
                crate::registry::validate_namespaced_keys("petramond:inherit", &keys)?;
                keys
            }
        };
        self.data = data;
        self.inherit = inherit;
        Ok(())
    }

    pub fn craftable_with(&self, inventory: &Inventory) -> bool {
        super::plan::plan(self, inventory).is_some()
    }

    pub fn from_data(data: CraftingRecipeData) -> Result<Self, String> {
        let station = CraftingStation::from_key(&data.station)
            .ok_or_else(|| format!("unknown crafting station '{}'", data.station))?;
        let result_item = item_by_key(&data.result.item)
            .ok_or_else(|| format!("unknown result item '{}'", data.result.item))?;
        let mut ingredients = Vec::with_capacity(data.ingredients.len());
        for ingredient in data.ingredients {
            let selector = match ingredient.selector {
                CraftingSelectorData::Item(key) => IngredientSelector::Item(
                    item_by_key(&key).ok_or_else(|| format!("unknown ingredient item '{key}'"))?,
                ),
                CraftingSelectorData::Tag(key) => IngredientSelector::Tag(
                    ItemTag::resolve(&key)
                        .map_err(|e| format!("unknown ingredient tag '{key}': {e}"))?,
                ),
            };
            let use_mode = match ingredient.use_mode {
                IngredientUseData::Consume => IngredientUse::Consume,
                IngredientUseData::Keep => IngredientUse::Keep,
                IngredientUseData::Remainder(key) => IngredientUse::Remainder(
                    item_by_key(&key).ok_or_else(|| format!("unknown remainder item '{key}'"))?,
                ),
            };
            ingredients.push(CraftingIngredient {
                selector,
                count: ingredient.count,
                use_mode,
            });
        }
        let mut recipe = Self::try_new(
            data.recipe,
            station,
            ingredients,
            ItemStack::new(result_item, data.result.count),
        )?;
        recipe.set_data(data.data)?;
        Ok(recipe)
    }

    pub fn to_data(&self) -> CraftingRecipeData {
        CraftingRecipeData {
            recipe: self.key.clone(),
            station: self.station.key().to_owned(),
            ingredients: self
                .ingredients
                .iter()
                .map(|ingredient| CraftingIngredientData {
                    selector: match ingredient.selector {
                        IngredientSelector::Item(item) => {
                            CraftingSelectorData::Item(item.key().to_owned())
                        }
                        IngredientSelector::Tag(tag) => {
                            CraftingSelectorData::Tag(public_tag_key(tag))
                        }
                    },
                    count: ingredient.count,
                    use_mode: match ingredient.use_mode {
                        IngredientUse::Consume => IngredientUseData::Consume,
                        IngredientUse::Keep => IngredientUseData::Keep,
                        IngredientUse::Remainder(item) => {
                            IngredientUseData::Remainder(item.key().to_owned())
                        }
                    },
                })
                .collect(),
            result: CraftingStackData {
                item: self.result.item.key().to_owned(),
                count: self.result.count,
            },
            data: self.data.clone(),
        }
    }
}

/// Name-addressed immutable crafting catalog data sent once at join. Registry
/// names, rather than session-local numeric ids, make this remap-free.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftingRecipeData {
    pub recipe: String,
    pub station: String,
    pub ingredients: Vec<CraftingIngredientData>,
    pub result: CraftingStackData,
    /// The row's compiled consumer-data entries — shipped so a client
    /// evaluates `petramond:inherit` (browser craftability) identically.
    #[serde(default)]
    pub data: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftingIngredientData {
    pub selector: CraftingSelectorData,
    pub count: u16,
    pub use_mode: IngredientUseData,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CraftingSelectorData {
    Item(String),
    Tag(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IngredientUseData {
    Consume,
    Keep,
    Remainder(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CraftingStackData {
    pub item: String,
    pub count: u8,
}

/// Ordered player-crafting recipes plus stable-key lookup.
#[derive(Clone, Default)]
pub struct CraftingCatalog {
    list: Vec<CraftingRecipe>,
    by_key: HashMap<String, usize>,
}

impl CraftingCatalog {
    pub fn new(list: Vec<CraftingRecipe>) -> Self {
        let mut kept = Vec::with_capacity(list.len());
        let mut by_key = HashMap::with_capacity(list.len());
        for recipe in list {
            if by_key.contains_key(recipe.key()) {
                log::error!("skipping duplicate crafting recipe key '{}'", recipe.key());
                continue;
            }
            let index = kept.len();
            by_key.insert(recipe.key().to_owned(), index);
            kept.push(recipe);
        }
        Self { list: kept, by_key }
    }

    pub fn iter(&self) -> impl Iterator<Item = &CraftingRecipe> {
        self.list.iter()
    }

    pub fn get(&self, key: &str) -> Option<&CraftingRecipe> {
        self.by_key.get(key).and_then(|index| self.list.get(*index))
    }

    pub fn get_at(&self, key: &str, station: CraftingStation) -> Option<&CraftingRecipe> {
        self.get(key)
            .filter(|recipe| station.admits(recipe.station()))
    }

    pub fn at(&self, station: CraftingStation) -> impl Iterator<Item = &CraftingRecipe> {
        self.list
            .iter()
            .filter(move |recipe| station.admits(recipe.station()))
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn to_data(&self) -> Vec<CraftingRecipeData> {
        self.list.iter().map(CraftingRecipe::to_data).collect()
    }

    pub fn from_data(data: Vec<CraftingRecipeData>) -> Self {
        let mut recipes = Vec::with_capacity(data.len());
        for row in data {
            let key = row.recipe.clone();
            match CraftingRecipe::from_data(row) {
                Ok(recipe) => recipes.push(recipe),
                Err(error) => log::error!("ignoring joined crafting recipe '{key}': {error}"),
            }
        }
        Self::new(recipes)
    }
}

/// A machine-processing recipe, looked up by `(class, input)` and IDENTIFIED
/// by its namespaced `recipe` key — the same identity a crafting row has, so
/// the one patch surface reaches both kinds of row.
#[derive(Clone, Debug)]
pub struct ProcessingRecipe {
    pub key: String,
    pub class: String,
    pub input: ItemType,
    pub result: ItemStack,
}

/// Every loaded recipe interaction model.
#[derive(Clone, Default)]
pub struct Recipes {
    crafting: CraftingCatalog,
    /// Keyed hash index over the processing rows (class → input → result) —
    /// `process` runs per machine tick and per mod `RecipeResult` call,
    /// never a linear scan. First row per `(class, input)` wins, like the
    /// old scan.
    processing: std::collections::HashMap<String, std::collections::HashMap<ItemType, ItemStack>>,
}

impl Recipes {
    pub fn new(crafting: Vec<CraftingRecipe>, processing: Vec<ProcessingRecipe>) -> Self {
        let mut index: std::collections::HashMap<String, std::collections::HashMap<_, _>> =
            std::collections::HashMap::new();
        for recipe in processing {
            index
                .entry(recipe.class)
                .or_default()
                .entry(recipe.input)
                .or_insert(recipe.result);
        }
        Self {
            crafting: CraftingCatalog::new(crafting),
            processing: index,
        }
    }

    pub fn crafting(&self) -> &CraftingCatalog {
        &self.crafting
    }

    pub fn len(&self) -> usize {
        self.crafting.len()
    }

    pub fn is_empty(&self) -> bool {
        self.crafting.is_empty()
    }

    pub fn process(&self, class: &str, input: ItemType) -> Option<ItemStack> {
        self.processing.get(class)?.get(&input).copied()
    }

    pub fn smelt(&self, input: ItemType) -> Option<ItemStack> {
        self.process(SMELTING_CLASS, input)
    }
}

fn item_by_key(key: &str) -> Option<ItemType> {
    ItemType::by_key(key)
}

fn public_tag_key(tag: ItemTag) -> String {
    let name = tag.name();
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("petramond:{name}")
    }
}

#[cfg(test)]
mod tests;
