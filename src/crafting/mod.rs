//! Data-driven crafting and processing recipes.

mod load;
mod plan;
mod recipe;
mod station;

pub use load::load_recipes_for;
pub use plan::{craft, output_accepts, CraftFailure};
pub(crate) use recipe::CraftingRecipeData;
pub use recipe::{CraftingCatalog, CraftingRecipe, IngredientSelector, IngredientUse, Recipes};
#[cfg(test)]
pub use recipe::{CraftingIngredient, ProcessingRecipe, SMELTING_CLASS};
pub use station::CraftingStation;
