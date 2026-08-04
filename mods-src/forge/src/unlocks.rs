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

/// Anything the player pulls out of a fresh ore vein.
const RAW_ORE_TAG: &str = "petramond:raw_ore";

const FORGING_FURNACE: &str = "forge:forging_furnace";
const POTTERY_TABLE: &str = "forge:pottery_table";

#[derive(Default)]
pub struct Unlocks {
    /// (item whose first-ever acquisition earns it, recipe opened).
    triggers: Vec<(ItemId, &'static str)>,
}

impl Unlocks {
    pub fn resolve() -> Unlocks {
        let mut triggers: Vec<(ItemId, &'static str)> = items_by_tag(RAW_ORE_TAG)
            .into_iter()
            .map(|ore| (ore, FORGING_FURNACE))
            .collect();
        for (name, recipe) in [
            ("petramond:clay", POTTERY_TABLE),
            (FORGING_FURNACE, POTTERY_TABLE),
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
