//! What a finished dish LEAVES BEHIND: the bowl the stew was served in.
//!
//! A container is what a RECIPE put the food in, so which dish returns which
//! vessel is this pack's business and not the engine's — no `food.remainder`
//! vocabulary was added there for it. The engine publishes the PRIMITIVE
//! instead: `item_used` names the acting player and WHICH use fired
//! (`ItemUseEvent::Eaten`), which is everything a policy like this needs.
//!
//! The eat has already finished when this runs — the portion is off the stack
//! and its effects have landed — so the give is unconditional and can never
//! cancel a meal. `GiveItemTo` addresses the EATER explicitly rather than an
//! implicit acting session, and drops at their feet whatever will not fit, so
//! a full inventory costs the player nothing.

use mod_sdk::*;

/// One dish and the vessel it is served in. Adding a dish is ONE row; nothing
/// here branches on a concrete item.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VesselSpec {
    returns: String,
}

const VESSEL_KEY: &str = "kitchen:vessel";

/// Dish rows resolved by the registry. An unknown returned item is logged
/// and skipped: the vessel is a courtesy, not a machine.
#[derive(Default)]
pub struct Vessels {
    rows: Vec<(ItemId, String)>,
}

impl Vessels {
    pub fn init(&mut self) {
        for (dish, spec) in items_with_data_as::<VesselSpec>(VESSEL_KEY) {
            if resolve_item(&spec.returns).is_some() {
                self.rows.push((dish, spec.returns));
            } else {
                log(&format!("kitchen: unknown vessel '{}'", spec.returns));
            }
        }
    }

    pub fn on_item_used(&self, player: PlayerId, item: ItemId, kind: ItemUseEvent) {
        if kind != ItemUseEvent::Eaten {
            return;
        }
        for (dish, returns) in &self.rows {
            if *dish == item {
                give_item_to(player, returns, 1, &[]);
            }
        }
    }
}

#[cfg(test)]
mod row_tests {
    use super::*;

    #[test]
    fn shipped_stew_declares_its_returned_bowl() {
        let rows = pack_rows_with_data(include_str!("../pack/items.json"), "items", VESSEL_KEY);
        assert_eq!(rows.len(), 1);
        let (dish, raw) = &rows[0];
        assert_eq!(dish, "kitchen:rabbit_stew");
        let spec: VesselSpec = parse_row_data(raw).unwrap();
        assert_eq!(spec.returns, "kitchen:wooden_bowl");
    }
}
