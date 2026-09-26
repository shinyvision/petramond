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
use weather_core::FieldParams;

use crate::content::Content;
use crate::farmland::{self, Hydration};
use crate::keys;

/// The hoe's claim GATE, run by both instances (see [`crate::claims`]): the
/// held hoe, eligible soil, and a clearable (or absent) cover above it.
/// Answers the cover to clear. Unloaded / mid-stream reads mean "not
/// actionable now" — quiet no-op. The hydration probe is not part of the
/// claim, only of which farmland appearance lands.
pub fn gate(
    content: &Content,
    world: &impl WorldView,
    item: ItemId,
    pos: [i32; 3],
    block: BlockId,
) -> Option<BlockId> {
    if item != content.iron_hoe {
        return None;
    }
    if block != content.grass && block != content.dirt && block != content.grass_fertilized {
        // Mud, sand, slabs, modded soil… all ineligible in 0.1.
        return None;
    }
    // Fertilized grass tills like grass — into PLAIN farmland: its fertility
    // was the spreading kind, not the soil upgrade. The player's choice to
    // cut a fertilizing lawn short must never brick the block.
    let cover = world.block([pos[0], pos[1] + 1, pos[2]])?;
    (cover == BlockId::AIR || content.is_clearable_cover(cover)).then_some(cover)
}

/// Till a cell the [`gate`] passed (server). `sky` is the weather field
/// heard this tick (`None` = clear sky): rain on open ground tills straight
/// to wet farmland.
pub fn till(content: &Content, sky: Option<&FieldParams>, pos: [i32; 3], cover: BlockId) -> Outcome {
    let above = [pos[0], pos[1] + 1, pos[2]];
    // Till: clear replaceable cover (it drops nothing, like being replaced by
    // a placement), then choose the best-known appearance immediately. An
    // Unknown probe starts dry; reconciliation catches up.
    if cover != BlockId::AIR {
        set_block(above, BlockId::AIR);
    }
    let soil = match farmland::probe(content, sky, pos) {
        Hydration::Hydrated => content.farmland_wet,
        Hydration::Dry | Hydration::Unknown => content.farmland_dry,
    };
    set_block(pos, soil);
    let center = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 1.0,
        pos[2] as f64 + 0.5,
    ];
    emit_sound(keys::TILL_SOUND, Some(center));
    emitter_burst(keys::TILL_BURST, center, 1.0);
    Outcome::Cancel
}
