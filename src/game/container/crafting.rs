use super::ContainerMenu;
use crate::crafting::{CraftFailure, Recipes};
use crate::inventory::{stack_onto_cursor, Inventory};
use crate::item::ItemStack;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum CraftMenuFailure {
    InvalidRecipe,
    OutputOccupied,
    MissingIngredients,
}

impl ContainerMenu {
    /// Revalidate and execute one stable-key recipe against the authoritative
    /// inventory. Selection is client-only; the session owns only this output.
    /// `bulk` (shift-craft) repeats until ingredients run out or the output
    /// stack fills; the first craft's failure is the request's outcome.
    pub(crate) fn craft_recipe(
        &mut self,
        inventory: &mut Inventory,
        recipes: &Recipes,
        recipe_key: &str,
        bulk: bool,
    ) -> Result<Vec<ItemStack>, CraftMenuFailure> {
        let station = self
            .crafting_station()
            .ok_or(CraftMenuFailure::InvalidRecipe)?;
        let recipe = recipes
            .crafting()
            .get_at(recipe_key, station)
            .ok_or(CraftMenuFailure::InvalidRecipe)?;
        let mut overflow = crate::crafting::craft(recipe, inventory, &mut self.craft_output)
            .map_err(|error| match error {
                CraftFailure::OutputOccupied => CraftMenuFailure::OutputOccupied,
                CraftFailure::MissingIngredients => CraftMenuFailure::MissingIngredients,
            })?;
        if bulk {
            // Bounded: every round consumes ingredients and the output stack
            // caps at the result's max stack size.
            while let Ok(more) = crate::crafting::craft(recipe, inventory, &mut self.craft_output) {
                overflow.extend(more);
            }
        }
        Ok(overflow)
    }

    /// Take the real output. Shift moves it into inventory; an ordinary click
    /// moves the whole stack onto a compatible cursor. Failed fits are no-ops.
    pub(super) fn craft_take_output(&mut self, inventory: &mut Inventory, shift: bool) {
        let Some(stack) = self.craft_output else {
            return;
        };
        if shift {
            if inventory.can_add(stack) {
                self.craft_output = inventory.add(stack);
            }
            return;
        }
        if stack_onto_cursor(inventory.cursor_mut(), stack) {
            self.craft_output = None;
        }
    }
}
