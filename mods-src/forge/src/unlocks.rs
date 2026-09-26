//! When the forge ladder becomes VISIBLE.
//!
//! The engine's default rule opens a recipe once the player has held every one
//! of its ingredients. That is a fine floor and a poor reveal: it puts the
//! whole pack behind items the player has no reason to gather yet (stone you
//! must think to smelt back out of cobble), so a player who has just struck
//! their first iron sees no way to do anything with it.
//!
//! So the pack states what EARNS each station instead, and the earning is the
//! moment of intent:
//!
//! - striking ore for the first time earns the furnace that melts it;
//! - a first lump of clay — or owning the furnace, which is useless without a
//!   mould — earns the pottery table that shapes the moulds.
//!
//! Ore is a TAG query, never three item names: an ore shipped by another pack
//! opens the furnace by carrying `petramond:raw_ore`, with nothing to change
//! here. These only ever open a recipe EARLIER than the default rule would.

use mod_sdk::*;

use crate::keys;

#[derive(Default)]
pub struct Unlocks {
    /// (item whose first-ever acquisition earns it, recipe opened).
    triggers: Vec<(ItemId, &'static str)>,
}

impl Unlocks {
    pub fn resolve() -> Unlocks {
        let mut triggers: Vec<(ItemId, &'static str)> = items_by_tag(keys::RAW_ORE_TAG)
            .into_iter()
            .map(|ore| (ore, keys::FORGING_FURNACE_RECIPE))
            .collect();
        // A first diamond earns the anvil: with the vanilla diamond tool
        // recipes retired, the diamond in hand has no visible use until the
        // player learns where augments are fitted.
        for (name, recipe) in [
            (keys::CLAY_ITEM, keys::POTTERY_TABLE_RECIPE),
            (keys::FORGING_FURNACE_ITEM, keys::POTTERY_TABLE_RECIPE),
            (keys::DIAMOND, keys::ANVIL_RECIPE),
        ] {
            if let Some(item) = resolve_item_logged(name) {
                triggers.push((item, recipe));
            }
        }
        Unlocks { triggers }
    }

    pub fn on_item_obtained(&self, player: PlayerId, item: ItemId) {
        for (trigger, recipe) in &self.triggers {
            if *trigger == item {
                unlock_recipe(player, recipe);
            }
        }
    }
}
