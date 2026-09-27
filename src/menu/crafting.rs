use super::ContainerMenu;
use petramond_world::crafting::{CraftFailure, Recipes};
use petramond_world::gui_state::PointerButton;
use petramond_world::inventory::Inventory;
use petramond_world::item::ItemStack;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CraftMenuFailure {
    InvalidRecipe,
    OutputOccupied,
    MissingIngredients,
}

impl ContainerMenu {
    pub fn craft_recipe(
        &mut self,
        inventory: &mut Inventory,
        recipes: &Recipes,
        progression: &crate::player::Progression,
        recipe_key: &str,
        bulk: bool,
    ) -> Result<Vec<ItemStack>, CraftMenuFailure> {
        let station = self
            .crafting_station()
            .ok_or(CraftMenuFailure::InvalidRecipe)?;
        if !progression.is_unlocked(recipe_key) {
            return Err(CraftMenuFailure::InvalidRecipe);
        }
        let recipe = recipes
            .crafting()
            .get_at(recipe_key, station)
            .ok_or(CraftMenuFailure::InvalidRecipe)?;
        let mut overflow =
            petramond_world::crafting::craft(recipe, inventory, &mut self.craft_output).map_err(
                |error| match error {
                    CraftFailure::OutputOccupied => CraftMenuFailure::OutputOccupied,
                    CraftFailure::MissingIngredients => CraftMenuFailure::MissingIngredients,
                },
            )?;
        if bulk {
            while let Ok(more) =
                petramond_world::crafting::craft(recipe, inventory, &mut self.craft_output)
            {
                overflow.extend(more);
            }
        }
        Ok(overflow)
    }

    pub(super) fn craft_take_output(
        &mut self,
        inventory: &mut Inventory,
        button: PointerButton,
        shift: bool,
    ) {
        let Some(stack) = self.craft_output else {
            return;
        };
        if shift {
            if inventory.can_add(stack) {
                self.craft_output = inventory.add(stack);
            }
            return;
        }
        inventory.click_take_only_external_slot(
            &mut self.craft_output,
            button == PointerButton::Secondary,
        );
    }
}
