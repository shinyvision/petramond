//! The water trough: right-click with a water bucket to fill it, or with an
//! empty bucket to drain it. Right-clicking the empty trough with at least
//! three wheat packs it with feed instead ([`FILL_WHEAT`] units — a full
//! trough is that many meals' worth). Sneak-right-clicking a wheat trough
//! with an empty hand takes the remaining feed back out. The held bucket
//! swaps in place so it stays in the player's hand.

use mod_sdk::*;

use crate::content::Content;

/// Filling a trough costs this much wheat — the herd-feeding store the
/// husbandry meals draw down (derived: a full trough is exactly
/// [`crate::husbandry::TROUGH_MEALS`] meals at
/// [`crate::husbandry::MEALS_PER_WHEAT`] meals per wheat).
pub const FILL_WHEAT: u32 =
    (crate::husbandry::TROUGH_MEALS / crate::husbandry::MEALS_PER_WHEAT) as u32;

pub fn on_item_use(content: &Content, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };

    if block == content.trough && item == content.water_bucket {
        if !replace_held_one(content.water_bucket, "petramond:wooden_bucket") {
            return Outcome::Continue;
        }
        swap_model_block(pos, content.trough_filled);
        // Fresh water holds fresh sips (cell KV rides the swap).
        crate::husbandry::clear_sips(content, pos);
        emit_sound("petramond:water_splash_small", Some(center(pos)));
        return Outcome::Cancel;
    }

    if block == content.trough_filled && item == content.wooden_bucket {
        if !replace_held_one(content.wooden_bucket, "petramond:water_bucket") {
            return Outcome::Continue;
        }
        swap_model_block(pos, content.trough);
        // Collected water can't leave a stale sip count behind.
        crate::husbandry::clear_sips(content, pos);
        emit_sound("petramond:water_splash_small", Some(center(pos)));
        return Outcome::Cancel;
    }

    // Wheat on the EMPTY trough packs it with feed — but only a full bundle
    // of three (the consume is atomic, so a smaller stack just falls
    // through). The empty trough never carries a meal count, so there is
    // nothing to scrub here.
    if block == content.trough && item == content.wheat_item {
        if !consume_held(content.wheat_item, FILL_WHEAT) {
            return Outcome::Continue;
        }
        swap_model_block(pos, content.trough_wheat);
        emit_sound("farming:harvest", Some(center(pos)));
        return Outcome::Cancel;
    }

    Outcome::Continue
}

/// Sneak + empty hand on a wheat trough takes the feed back out: the trough
/// swaps to empty and the player gets the un-eaten wheat — one per
/// [`crate::husbandry::MEALS_PER_WHEAT`] meals REMAINING, floored (the flock's
/// partial nibbles are lost). Any other click — not sneaking, or something
/// in hand — falls through so placement and the fill paths still see it.
pub fn on_interact(
    content: &Content,
    pos: [i32; 3],
    block: BlockId,
    item: Option<ItemId>,
) -> Outcome {
    if block != content.trough_wheat || item.is_some() {
        return Outcome::Continue;
    }
    if !acting_sneaks() {
        return Outcome::Continue;
    }
    let meals = crate::husbandry::meals_at(pos);
    let back = crate::husbandry::wheat_yield(meals);
    if back > 0 {
        give_item("farming:wheat", back);
    }
    swap_model_block(pos, content.trough);
    // The swap carries cell KV across — an emptied trough must not bank a
    // stale meal count (the sip pattern).
    crate::husbandry::clear_meals(content, pos);
    emit_sound("farming:harvest", Some(center(pos)));
    Outcome::Cancel
}

/// The click initiator's sneak state. `block_interact` carries no player (the
/// frozen per-acting-session ABI), so: `player_state` IS the acting session's
/// snapshot — find the roster entry sharing its exact look, then read that
/// player's published input. Yaw+pitch are the fingerprint: pos/vel/on_ground
/// drift between the roster's pre-movement publish and the placement stage's
/// post-movement read. An ambiguous match refuses rather than guess.
fn acting_sneaks() -> bool {
    let me = player_state();
    let mut hit = None;
    for p in players() {
        if p.state.yaw == me.yaw && p.state.pitch == me.pitch {
            if hit.is_some() {
                return false;
            }
            hit = Some(p.id);
        }
    }
    hit.and_then(player_input).is_some_and(|i| i.sneak)
}

fn center(pos: [i32; 3]) -> [f32; 3] {
    [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.5,
        pos[2] as f32 + 0.5,
    ]
}
