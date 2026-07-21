//! The compost barrel: surplus produce becomes fertilizer.
//!
//! FILL rides `item_use_pre`: right-clicking a non-full barrel with any item
//! carrying the `farming:compostable` tag consumes one unit and advances the
//! barrel one fill stage. Stage identity IS the block id (the crop pattern) —
//! no cell KV, and the barrel's look is always honest about its fill: the
//! four stages are `models.json` rows over ONE composter `.bbmodel`, the
//! empty row hiding the `compost_surface` cube and each fill posing it
//! higher via `part_offsets`. Stage flips are same-footprint
//! `swap_model_block`s (the kitchen lit-machine mechanism), never plain
//! block writes. COLLECT rides `block_interact`: any click on a FULL barrel
//! pops one fertilizer and resets it to empty — the same pop-and-reset
//! ergonomics as a mature crop harvest, working with an empty hand or any
//! held item.
//!
//! What the popped fertilizer then DOES to a block is [`crate::fertilize`]'s
//! business — its own link in the lib.rs item-use chain.

use mod_sdk::*;

use crate::content::Content;

/// One compostable unit advances a non-full barrel one fill stage. The held
/// item is checked before any host crossing (the tilling.rs order): every
/// other right-click costs nothing here.
pub fn on_item_use(content: &Content, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
    if !content.compostable.contains(&item) {
        return Outcome::Continue;
    }
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    // Unloaded / mid-stream reads mean "not actionable now" — quiet no-op.
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };
    let Some(stage @ 0..=2) = content.compost_stage(block) else {
        return Outcome::Continue;
    };
    if !consume_held(item, 1) {
        return Outcome::Continue;
    }
    swap_model_block(pos, content.compost[stage as usize + 1]);
    let center = barrel_top(pos);
    emit_sound("farming:till", Some(center));
    emitter_burst("farming:compost_fill", center, 1.0);
    Outcome::Cancel
}

/// Any right click on a FULL barrel pops one fertilizer and resets it.
/// Non-full barrels don't consume the click — the fill path (or ordinary
/// placement against the barrel) still sees it.
pub fn on_interact(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    if content.compost_stage(block) != Some(3) {
        return Outcome::Continue;
    }
    let center = barrel_top(pos);
    spawn_item("farming:fertilizer", 1, center);
    swap_model_block(pos, content.compost[0]);
    emit_sound("farming:harvest", Some(center));
    emitter_burst("farming:compost_fill", center, 1.0);
    Outcome::Cancel
}

/// CLIENT prediction mirror of [`on_item_use`]'s gate.
pub fn predict_item_use(content: &Content, item: ItemId, block: BlockId) -> Outcome {
    if content.compostable.contains(&item) && matches!(content.compost_stage(block), Some(0..=2)) {
        Outcome::Cancel
    } else {
        Outcome::Continue
    }
}

/// CLIENT prediction mirror of [`on_interact`]'s gate.
pub fn predict_interact(content: &Content, block: BlockId) -> Outcome {
    if content.compost_stage(block) == Some(3) {
        Outcome::Cancel
    } else {
        Outcome::Continue
    }
}

fn barrel_top(pos: [i32; 3]) -> [f32; 3] {
    [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 1.2,
        pos[2] as f32 + 0.5,
    ]
}
