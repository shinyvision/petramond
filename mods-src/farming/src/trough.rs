use mod_sdk::*;

use crate::content::Content;
use crate::keys;

pub const FILL_WHEAT: u32 =
    (crate::husbandry::TROUGH_MEALS / crate::husbandry::MEALS_PER_WHEAT) as u32;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TroughUse {
    Fill,
    Drain,
    PackWheat,
}

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
            if !consume_held(content.wheat_item, FILL_WHEAT) {
                return Outcome::Continue;
            }
            swap_block(pos, content.trough_wheat);
            emit_sound(keys::HARVEST_SOUND, Some(block_center(pos)));
        }
    }
    Outcome::Cancel
}

pub fn take_out_gate(content: &Content, block: BlockId, actor: &PlayerSnapshot) -> bool {
    block == content.trough_wheat && actor.held.is_none() && actor.sneak
}

pub fn take_out(content: &Content, pos: [i32; 3]) -> Outcome {
    let meals = crate::husbandry::meals_at(pos);
    let back = crate::husbandry::wheat_yield(meals);
    if back > 0 {
        give_item(keys::WHEAT, back);
    }
    swap_block(pos, content.trough);
    crate::husbandry::clear_meals(content, pos);
    emit_sound(keys::HARVEST_SOUND, Some(block_center(pos)));
    Outcome::Cancel
}

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
