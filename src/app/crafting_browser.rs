//! Presentation-only state for the searchable player-crafting browser.
//!
//! Search, hover and selection deliberately stay out of the simulation. The
//! server owns only the immutable joined catalog, inventory, and transient
//! output; an explicit CRAFT request carries the selected stable recipe key.
//! The craftable-only filter is the one preference that leaves this module:
//! the toggle updates the game, which persists it in the world's player data.
//!
//! The browser is a GRID of result icons: what a recipe is called and what it
//! costs are revealed on HOVER (the floating tooltip), because a row per
//! recipe only ever showed a handful of a catalog that grows combinatorially
//! with material families. The selection is shown by the cell's own selected
//! face — spelling it out again in a detail line cost two grid rows, which is
//! the scrolling this grid exists to remove.

use std::sync::Arc;

use petramond_ui::{UiEvent, UiMap, UiState, UiValue};

use crate::crafting::CraftingStation;
use crate::game::Game;
use crate::gui::CraftingRecipeView;

/// The document id of the recipe grid, whose hovered stamp drives the tooltip.
pub(super) const RECIPE_LIST_ID: &str = "craft_recipes_list";

#[derive(Default)]
pub(super) struct CraftingBrowser {
    search: String,
    selected: Option<String>,
    visible: Vec<VisibleRecipe>,
    rows: Arc<Vec<UiMap>>,
    cache_key: Option<BrowserCacheKey>,
    /// Filtered-row index under the cursor, resolved each frame from the
    /// previous frame's hovered grid stamp.
    hovered: Option<usize>,
}

#[derive(PartialEq, Eq)]
struct BrowserCacheKey {
    station: CraftingStation,
    inventory_revision: u64,
    query: String,
    craftable_only: bool,
    /// How many recipes are unlocked. Unlocking only appends, so the count is
    /// a complete change signal — and a rebuild is exactly what a fresh
    /// unlock needs.
    unlocked: usize,
}

struct VisibleRecipe {
    key: String,
    view: CraftingRecipeView,
    craftable: bool,
    /// Result name plus its `×N` count — the tooltip and detail headline.
    label: String,
}

impl CraftingBrowser {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn views(&self) -> impl Iterator<Item = &CraftingRecipeView> {
        self.visible.iter().map(|row| &row.view)
    }

    /// The hovered recipe, drawn in the floating tooltip.
    pub(super) fn tip_view(&self) -> Option<&CraftingRecipeView> {
        self.visible.get(self.hovered?).map(|row| &row.view)
    }

    pub(super) fn populate(
        &mut self,
        game: &Game,
        station: CraftingStation,
        hovered: Option<usize>,
        state: &mut UiState,
    ) {
        let menu = game.menu_read_model();
        let inventory = menu.inventory;
        let craftable_only = game.craft_craftable_only();
        let query = self.search.trim().to_lowercase();
        let progression = game.progression();
        let next_key = BrowserCacheKey {
            station,
            inventory_revision: game.replicated_inventory_revision(),
            query,
            craftable_only,
            unlocked: progression.unlocked().len(),
        };
        if self.cache_key.as_ref() != Some(&next_key) {
            self.visible.clear();
            for recipe in game.crafting_catalog().at(station) {
                // A locked recipe is not shown at all — the point of unlocks
                // is that the catalog reveals itself as the player earns it.
                if !progression.is_unlocked(recipe.key()) {
                    continue;
                }
                let result = recipe.result();
                if !next_key.query.is_empty()
                    && !result.item.name().to_lowercase().contains(&next_key.query)
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
                let label = match result.count {
                    1 => result.item.name().to_owned(),
                    n => format!("{} \u{d7}{n}", result.item.name()),
                };
                self.visible.push(VisibleRecipe {
                    key: recipe.key().to_owned(),
                    view: CraftingRecipeView {
                        result: result.item,
                        ingredients,
                        craftable,
                    },
                    craftable,
                    label,
                });
            }
            // Craftable recipes lead the grid; the stable sort keeps joined
            // catalog order within each group, so material families stay
            // clustered instead of scattering across the cells.
            self.visible.sort_by_key(|row| !row.craftable);
            self.rows = Arc::new(
                self.visible
                    .iter()
                    .map(|row| {
                        let mut map = UiMap::new();
                        map.insert("name".into(), UiValue::Str(row.label.clone()));
                        map.insert("enabled".into(), UiValue::Bool(row.craftable));
                        map
                    })
                    .collect(),
            );
            self.cache_key = Some(next_key);
        }

        // A tooltip that follows the cursor would fight the held stack for the
        // same pixels, so a drag suppresses it.
        self.hovered = hovered
            .filter(|&index| index < self.visible.len())
            .filter(|_| inventory.cursor().is_none());
        let selected = self
            .selected
            .as_deref()
            .and_then(|key| self.visible.iter().position(|row| row.key == key));
        let can_craft = selected
            .and_then(|index| self.visible.get(index))
            .filter(|row| row.craftable)
            .is_some_and(|row| self.output_accepts(game, &row.key));

        // The station's screen title: its block item's display name (a pack
        // workbench key is its block item's key), or the engine table's.
        let title = crate::item::ItemType::by_key(station.key())
            .map(|item| item.name().to_owned())
            .unwrap_or_else(|| "Crafting Table".to_owned());
        state.set("craft_station_title", UiValue::Str(title));
        state.set("craft_search", UiValue::Str(self.search.clone()));
        state.set("craft_recipes", UiValue::List(self.rows.clone()));
        state.set(
            "craft_recipe_sel",
            UiValue::I32(selected.map(|index| index as i32).unwrap_or(-1)),
        );
        state.set("can_craft", UiValue::Bool(can_craft));
        state.set("craft_filter_on", UiValue::Bool(craftable_only));
        state.set("no_craft_results", UiValue::Bool(self.visible.is_empty()));
        // An empty grid means two different things now. A player who has
        // unlocked nothing at this station is not filtering badly — they have
        // not found anything yet, and saying "no matching recipes" to someone
        // who typed nothing reads as a bug.
        let unfiltered = self.search.trim().is_empty() && !craftable_only;
        state.set(
            "craft_empty_hint",
            UiValue::Str(
                if unfiltered {
                    "Gather materials to discover recipes"
                } else {
                    "No matching recipes"
                }
                .to_owned(),
            ),
        );

        let tip = self.hovered.and_then(|index| self.visible.get(index));
        state.set("show_recipe_tip", UiValue::Bool(tip.is_some()));
        state.set(
            "craft_tip_name",
            UiValue::Str(tip.map(|row| row.label.clone()).unwrap_or_default()),
        );
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
