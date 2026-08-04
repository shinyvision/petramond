//! Pack-authored recipe unlocks: the earlier, thematic triggers this pack
//! wants instead of the engine's default (hold EVERY ingredient).
//!
//! The Farmer's Workbench costs iron and a crafting table on top of wheat, so
//! the default rule keeps it invisible until the player has already tooled up
//! — long after they are farming and looking for somewhere to process a
//! harvest. Growing wheat is what earns it, so wheat is what opens it.
//!
//! The default rule still applies underneath; these only ever open a recipe
//! sooner.

use mod_sdk::*;

use crate::content::Content;

/// (trigger item, recipe opened the first time the player ever holds it).
fn triggers(content: &Content) -> [(ItemId, &'static str); 1] {
    [(content.wheat_item, "farming:farmers_workbench")]
}

pub fn on_item_obtained(content: &Content, player: PlayerId, item: ItemId) {
    for (trigger, recipe) in triggers(content) {
        if trigger == item {
            unlock_recipe(player, recipe);
        }
    }
}
