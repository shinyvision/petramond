//! Data-driven crafting, processing, and furniture recipes.

mod load;
mod plan;
mod recipe;
mod station;

#[cfg(test)]
pub use load::load_recipes;
pub use load::load_recipes_for;
pub use plan::{craft, output_accepts, CraftFailure};
pub(crate) use recipe::CraftingRecipeData;
pub use recipe::{CraftingCatalog, CraftingRecipe, IngredientSelector, IngredientUse, Recipes};
#[cfg(test)]
pub use recipe::{CraftingIngredient, FurnitureRecipe, ProcessingRecipe, SMELTING_CLASS};
pub use station::CraftingStation;
