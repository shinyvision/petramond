//! The iron hoe: turning grass/dirt into farmland.
//!
//! Handled through the generic `item_use_pre` event (the hoe's id resolved by
//! NAME at init — never a hardcoded registry number). An eligible use
//! replaces the target with the best-known dry/wet farmland variant, plays
//! the till crunch + dirt burst, and CANCELS the event (click consumed, hand
//! jab from the engine's used-item path). An ineligible target does not
//! cancel: the click falls through the ordinary ladder and, the hoe placing
//! nothing, quietly does nothing — no chat or sound spam, hoe untouched.

use mod_sdk::*;

use crate::content::Content;
use crate::farmland::{self, Hydration};

pub fn on_item_use(content: &Content, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
    if item != content.iron_hoe {
        return Outcome::Continue;
    }
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    // Unloaded / mid-stream reads mean "not actionable now" — quiet no-op.
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };
    if block != content.grass && block != content.dirt && block != content.grass_fertilized {
        // Mud, sand, slabs, modded soil… all ineligible in 0.1.
        return Outcome::Continue;
    }
    // Fertilized grass tills like grass — into PLAIN farmland: its fertility
    // was the spreading kind, not the soil upgrade. The player's choice to
    // cut a fertilizing lawn short must never brick the block.
    let above = [pos[0], pos[1] + 1, pos[2]];
    let Some(cover) = get_block(above) else {
        return Outcome::Continue;
    };
    if cover != BlockId::AIR && !content.is_clearable_cover(cover) {
        return Outcome::Continue;
    }
    // Till: clear replaceable cover (it drops nothing, like being replaced by
    // a placement), then choose the best-known appearance immediately. An
    // Unknown probe starts dry; reconciliation catches up.
    if cover != BlockId::AIR {
        set_block(above, BlockId::AIR);
    }
    let soil = match farmland::probe(content, pos) {
        Hydration::Hydrated => content.farmland_wet,
        Hydration::Dry | Hydration::Unknown => content.farmland_dry,
    };
    set_block(pos, soil);
    let center = [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 1.0,
        pos[2] as f32 + 0.5,
    ];
    emit_sound("farming:till", Some(center));
    emitter_burst("farming:till_burst", center, 1.0);
    Outcome::Cancel
}
