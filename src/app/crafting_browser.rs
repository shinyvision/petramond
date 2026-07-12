//! Presentation-only state for the searchable player-crafting browser.
//!
//! Search and selection deliberately stay out of the simulation. The server
//! owns only the immutable joined catalog, inventory, and transient output;
//! an explicit CRAFT request carries the selected stable recipe key. The
//! craftable-only filter is the one preference that leaves this module: the
//! toggle updates the game, which persists it in the world's player data.

use std::sync::Arc;

use petramond_ui::{UiEvent, UiMap, UiState, UiValue};

use crate::crafting::CraftingStation;
use crate::game::Game;
use crate::gui::CraftingRecipeView;

#[derive(Default)]
pub(super) struct CraftingBrowser {
    search: String,
    selected: Option<String>,
    visible: Vec<VisibleRecipe>,
    rows: Arc<Vec<UiMap>>,
    cache_key: Option<BrowserCacheKey>,
}

#[derive(PartialEq, Eq)]
struct BrowserCacheKey {
    station: CraftingStation,
    inventory_revision: u64,
    query: String,
    craftable_only: bool,
}

struct VisibleRecipe {
    key: String,
    view: CraftingRecipeView,
    craftable: bool,
}

impl CraftingBrowser {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn views(&self) -> impl Iterator<Item = &CraftingRecipeView> {
        self.visible.iter().map(|row| &row.view)
    }

    pub(super) fn populate(&mut self, game: &Game, station: CraftingStation, state: &mut UiState) {
        let menu = game.menu_read_model();
        let inventory = menu.inventory;
        let craftable_only = game.craft_craftable_only();
        let query = self.search.trim().to_lowercase();
        let next_key = BrowserCacheKey {
            station,
            inventory_revision: game.replicated_inventory_revision(),
            query,
            craftable_only,
        };
        if self.cache_key.as_ref() != Some(&next_key) {
            self.visible.clear();
            for recipe in game.crafting_catalog().at(station) {
                let result = recipe.result().item;
                if !next_key.query.is_empty()
                    && !result.name().to_lowercase().contains(&next_key.query)
                    && !recipe.key().to_lowercase().contains(&next_key.query)
                {
                    continue;
                }
                let craftable = recipe.craftable_with(inventory);
                if craftable_only && !craftable {
                    continue;
                }
                let ingredients = recipe
                    .ingredients()
                    .iter()
                    .filter_map(|ingredient| {
                        ingredient
                            .selector
                            .display_item(inventory)
                            .map(|item| (item, ingredient.count))
                    })
                    .collect();
                self.visible.push(VisibleRecipe {
                    key: recipe.key().to_owned(),
                    view: CraftingRecipeView {
                        result,
                        ingredients,
                        craftable,
                    },
                    craftable,
                });
            }
            // Craftable recipes lead the list; the stable sort keeps joined
            // catalog order within each group.
            self.visible.sort_by_key(|row| !row.craftable);
            self.rows = Arc::new(
                self.visible
                    .iter()
                    .map(|row| {
                        let mut map = UiMap::new();
                        map.insert(
                            "name".into(),
                            UiValue::Str(row.view.result.name().to_owned()),
                        );
                        map.insert("enabled".into(), UiValue::Bool(row.craftable));
                        map
                    })
                    .collect(),
            );
            self.cache_key = Some(next_key);
        }

        let selected = self
            .selected
            .as_deref()
            .and_then(|key| self.visible.iter().position(|row| row.key == key));
        let can_craft = selected
            .and_then(|index| self.visible.get(index))
            .filter(|row| row.craftable)
            .is_some_and(|row| self.output_accepts(game, &row.key));

        state.set("craft_search", UiValue::Str(self.search.clone()));
        state.set("craft_recipes", UiValue::List(self.rows.clone()));
        state.set(
            "craft_recipe_sel",
            UiValue::I32(selected.map(|index| index as i32).unwrap_or(-1)),
        );
        state.set("can_craft", UiValue::Bool(can_craft));
        state.set("craft_filter_on", UiValue::Bool(craftable_only));
        state.set("no_craft_results", UiValue::Bool(self.visible.is_empty()));
    }

    /// UI enablement mirror of the server's output rule: empty output, or the
    /// same item with room for one more full result.
    fn output_accepts(&self, game: &Game, key: &str) -> bool {
        game.crafting_catalog().get(key).is_some_and(|recipe| {
            crate::crafting::output_accepts(recipe, game.menu_read_model().craft_output)
        })
    }

    pub(super) fn handle(&mut self, game: &mut Game, event: &UiEvent, shift: bool) -> bool {
        match event {
            UiEvent::TextChanged { id, text } if id == "craft_search" => {
                self.search.clone_from(text);
                true
            }
            UiEvent::Toggle { id, on, .. } if id == "craft_filter" => {
                game.set_craft_craftable_only(*on);
                true
            }
            UiEvent::Click {
                id,
                item: Some(index),
                button: petramond_ui::PointerButton::Primary,
            } if id == "recipe" => {
                if let Some(row) = self
                    .visible
                    .get(*index as usize)
                    .filter(|row| row.craftable)
                {
                    self.selected = Some(row.key.clone());
                }
                true
            }
            UiEvent::Click {
                id,
                button: petramond_ui::PointerButton::Primary,
                ..
            } if id == "craft" => {
                let Some(key) = self.selected.clone() else {
                    return true;
                };
                let enabled = self
                    .visible
                    .iter()
                    .any(|row| row.key == key && row.craftable)
                    && self.output_accepts(game, &key);
                if enabled {
                    game.craft_recipe(&key, shift);
                }
                true
            }
            _ => false,
        }
    }
}
