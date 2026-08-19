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
struct VesselSpec {
    dish: &'static str,
    returns: &'static str,
}

const VESSELS: &[VesselSpec] = &[VesselSpec {
    dish: "kitchen:rabbit_stew",
    returns: "kitchen:wooden_bowl",
}];

/// The rows with their dish resolved to a session id. A row whose dish is
/// missing is dropped with a log rather than failing the mod: the vessel is a
/// courtesy, not a machine.
#[derive(Default)]
pub struct Vessels {
    rows: Vec<(ItemId, &'static str)>,
}

impl Vessels {
    pub fn init(&mut self) {
        for spec in VESSELS {
            match resolve_item(spec.dish) {
                Some(id) => self.rows.push((id, spec.returns)),
                None => log(&format!("kitchen: unknown dish '{}'", spec.dish)),
            }
        }
    }

    pub fn on_item_used(&self, player: PlayerId, item: ItemId, kind: ItemUseEvent) {
        if kind != ItemUseEvent::Eaten {
            return;
        }
        for &(dish, returns) in &self.rows {
            if dish == item {
                give_item_to(player, returns, 1, &[]);
            }
        }
    }
}
