//! CLIENT-side prediction: the same pre events the server dispatches
//! (`interact_attempt`, `item_use_pre`, `block_place_pre`), answered
//! speculatively against the REPLICA so the engine's jab / place-ghost
//! prediction sees this mod's claims exactly like an engine consumer's.
//!
//! Presentation-only mirrors: each predictor re-states the authoritative
//! handler's GATE (never its mutation) over replica reads
//! (`client_blocks_at`) and the actor snapshot (`player_state`). Cancel =
//! "I predict I claim this attempt" (interact/use) or "I predict I veto this
//! placement" (place_pre). Known divergences from the authoritative gates,
//! chosen over silent wrongness: no client light read (a dark-cave planting
//! predicts placeable and over-jabs), no hydration probe. When a gate here
//! drifts from its authoritative twin the cost is only feel (a false or
//! missed jab); the server outcome is unaffected.

use mod_sdk::*;

use crate::content::Content;
use crate::{chain, compost, crops, fertilize, tilling, trough};

/// One replica block read. `None` = unloaded / not stream-final — never a
/// claim: nothing here may claim a click it cannot inspect.
pub fn peek(pos: [i32; 3]) -> Option<BlockId> {
    client_blocks_at(vec![pos]).into_iter().next().flatten()
}

/// Predicted `interact_attempt` on a block target.
pub fn on_interact_attempt(content: &Content, pos: [i32; 3]) -> Outcome {
    let Some(block) = peek(pos) else {
        return Outcome::Continue;
    };
    let actor = player_state();
    let first = chain(crops::predict_interact(content, block, &actor), || {
        compost::predict_interact(content, block)
    });
    chain(first, || trough::predict_interact(content, block, &actor))
}

/// Predicted `item_use_pre` — the same handler order as the authoritative
/// dispatch (hoe, fertilizer, compost fill, trough).
pub fn on_item_use(content: &Content, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    let Some(block) = peek(pos) else {
        return Outcome::Continue;
    };
    let first = chain(tilling::predict_item_use(content, item, pos, block), || {
        fertilize::predict_item_use(content, item, pos, block)
    });
    let second = chain(first, || compost::predict_item_use(content, item, block));
    chain(second, || {
        trough::predict_item_use(content, item, block, player_state().held_count)
    })
}

/// Predicted `block_place_pre` (the seeds-on-grass case: a predicted Cancel
/// is a KNOWN refusal — no jab, no ghost).
pub fn on_place_pre(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    crops::predict_place_pre(content, pos, block)
}
