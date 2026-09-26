//! The water trough: right-click with a water bucket to fill it, or with an
//! empty bucket to drain it. Right-clicking the empty trough with at least
//! three wheat packs it with feed instead ([`FILL_WHEAT`] units — a full
//! trough is that many meals' worth). Sneak-right-clicking a wheat trough
//! with an empty hand takes the remaining feed back out. The held bucket
//! swaps in place so it stays in the player's hand.

use mod_sdk::*;

use crate::content::Content;
use crate::keys;

/// Filling a trough costs this much wheat — the herd-feeding store the
/// husbandry meals draw down (derived: a full trough is exactly
/// [`crate::husbandry::TROUGH_MEALS`] meals at
/// [`crate::husbandry::MEALS_PER_WHEAT`] meals per wheat).
pub const FILL_WHEAT: u32 =
    (crate::husbandry::TROUGH_MEALS / crate::husbandry::MEALS_PER_WHEAT) as u32;

/// What a held item does to a trough.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TroughUse {
    /// A water bucket fills the empty trough.
    Fill,
    /// An empty bucket drains the filled trough.
    Drain,
    /// A full bundle of wheat packs the empty trough with feed.
    PackWheat,
}

/// The item-use claim GATE, run by both instances (see [`crate::claims`]):
/// the bucket swaps, and the wheat fill only with a full bundle of
/// [`FILL_WHEAT`] in hand (the acting hand's count — the consume is atomic,
/// so a smaller stack falls through).
pub fn use_gate(content: &Content, item: ItemId, block: BlockId) -> Option<TroughUse> {
    if let Some((swap, _)) = bucket_swap(content, block, item) {
        return Some(match swap {
            BucketSwap::Pour => TroughUse::Fill,
            BucketSwap::Scoop => TroughUse::Drain,
        });
    }
    let bundle = || u32::from(player_state().held_count) >= FILL_WHEAT;
    (block == content.trough && item == content.wheat_item && bundle())
        .then_some(TroughUse::PackWheat)
}

/// Apply a gated trough use (server). The held bucket swaps in place.
pub fn apply(content: &Content, pos: [i32; 3], using: TroughUse) -> Outcome {
    match using {
        TroughUse::Fill => {
            if !content.buckets.perform(BucketSwap::Pour, pos, || {
                swap_block(pos, content.trough_filled);
                crate::husbandry::clear_sips(content, pos);
            }) {
                return Outcome::Continue;
            }
        }
        TroughUse::Drain => {
            if !content.buckets.perform(BucketSwap::Scoop, pos, || {
                swap_block(pos, content.trough);
                crate::husbandry::clear_sips(content, pos);
            }) {
                return Outcome::Continue;
            }
        }
        TroughUse::PackWheat => {
            // The empty trough never carries a meal count, so there is
            // nothing to scrub here.
            if !consume_held(content.wheat_item, FILL_WHEAT) {
                return Outcome::Continue;
            }
            swap_block(pos, content.trough_wheat);
            emit_sound(keys::HARVEST_SOUND, Some(block_center(pos)));
        }
    }
    Outcome::Cancel
}

/// The take-out claim GATE, run by both instances: sneak + empty hand on a
/// wheat trough. Any other click — not sneaking, or something in hand —
/// falls through so placement and the fill paths still see it.
pub fn take_out_gate(content: &Content, block: BlockId, actor: &PlayerSnapshot) -> bool {
    block == content.trough_wheat && actor.held.is_none() && actor.sneak
}

/// Take a gated wheat trough's feed back out (server): the trough swaps to
/// empty and the player gets the un-eaten wheat — one per
/// [`crate::husbandry::MEALS_PER_WHEAT`] meals REMAINING, floored (the
/// flock's partial nibbles are lost).
pub fn take_out(content: &Content, pos: [i32; 3]) -> Outcome {
    let meals = crate::husbandry::meals_at(pos);
    let back = crate::husbandry::wheat_yield(meals);
    if back > 0 {
        give_item(keys::WHEAT, back);
    }
    swap_block(pos, content.trough);
    // The swap carries cell KV across — an emptied trough must not bank a
    // stale meal count (the sip pattern).
    crate::husbandry::clear_meals(content, pos);
    emit_sound(keys::HARVEST_SOUND, Some(block_center(pos)));
    Outcome::Cancel
}

/// The bucket swap `item` offers on trough cell `block`, with the block the
/// trough becomes: a water bucket fills the empty trough, an empty bucket
/// drains the filled one.
fn bucket_swap(content: &Content, block: BlockId, item: ItemId) -> Option<(BucketSwap, BlockId)> {
    let (full, next) = if block == content.trough {
        (false, content.trough_filled)
    } else if block == content.trough_filled {
        (true, content.trough)
    } else {
        return None;
    };
    Some((content.buckets.swap_for(item, full)?, next))
}
